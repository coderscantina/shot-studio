//! What `shot check` measures in an export, and the frame access thumbnails share.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferGetBaseAddressOfPlane, CVPixelBufferGetBytesPerRowOfPlane,
    CVPixelBufferGetHeightOfPlane, CVPixelBufferGetWidthOfPlane, CVPixelBufferLockBaseAddress,
    CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
};
use shot_audio::Meter;
use shot_core::{Micros, Project, Timeline};
use shot_media::audio::{PcmReader, SAMPLE_RATE};
use shot_media::videotoolbox::Decoder;

use crate::check::{self, CheckKind, FrameSample, Problem};
use crate::spoken::{chapter_marks, duration, spoken_words};

/// Frames are sampled this often. The export has a keyframe at least every 0.5 s, so each sample
/// decodes at most one chunk.
pub const FRAME_STEP: Micros = Micros(500_000);
/// The grid of block means compared for frozen frames.
const GRID: (usize, usize) = (16, 9);

#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("{0}")]
    Audio(String),
    #[error("{0}")]
    Video(String),
}

/// Integrated loudness and true peak of a file's audio, as the voice chain's finish measures it.
pub fn loudness(path: &Path) -> Result<(Option<f64>, f64), MediaError> {
    let audio =
        |e: shot_media::audio::AudioError| MediaError::Audio(format!("{}: {e}", path.display()));
    let mut reader = PcmReader::open(path).map_err(audio)?;
    let mut meter = Meter::new();
    let block = u64::from(SAMPLE_RATE);
    let mut samples = Vec::with_capacity(block as usize);
    let mut at = 0;
    loop {
        samples.clear();
        reader.read(at, at + block, &mut samples).map_err(audio)?;
        // The reader pads past the end with silence; a block of digital silence after the
        // first is the end of the track.
        if at > 0 && samples.iter().all(|s| *s == 0.0) {
            break;
        }
        meter.add(&samples);
        at += block;
    }
    Ok((meter.loudness(), meter.true_peak()))
}

/// Samples the video every [`FRAME_STEP`] up to `length` or its own end.
pub fn sample_frames(path: &Path, length: Micros) -> Result<Vec<FrameSample>, MediaError> {
    let video = |e: shot_media::videotoolbox::DecodeError| {
        MediaError::Video(format!("{}: {e}", path.display()))
    };
    let mut decoder = Decoder::open_native(path).map_err(video)?;
    // Seeking past the end shows the last frame, which would read as frozen.
    let end = length.0.min(decoder.duration().as_micros() as u64);
    let mut samples = Vec::new();
    let mut at = 0;
    while at < end {
        let Some(frame) = decoder
            .seek(Duration::from_micros(at), || false)
            .map_err(video)?
        else {
            break;
        };
        let planes = Planes::lock(&frame.buffer);
        samples.push(FrameSample {
            at: Micros(at),
            luma: planes.mean(),
            grid: planes.grid(GRID),
        });
        at += FRAME_STEP.0;
    }
    Ok(samples)
}

/// A read-only lock on a decoded 4:2:0 bi-planar frame.
pub struct Planes<'a> {
    buffer: &'a CVPixelBuffer,
    pub width: usize,
    pub height: usize,
    luma: &'a [u8],
    luma_stride: usize,
    chroma: &'a [u8],
    chroma_stride: usize,
}

impl<'a> Planes<'a> {
    pub fn lock(buffer: &'a CVPixelBuffer) -> Self {
        // SAFETY: the buffer is a decoded bi-planar frame. While locked read-only, each plane's
        // base address is valid for stride * height bytes, and `Drop` unlocks it.
        unsafe {
            CVPixelBufferLockBaseAddress(buffer, CVPixelBufferLockFlags::ReadOnly);
            let plane = |index: usize| {
                let stride = CVPixelBufferGetBytesPerRowOfPlane(buffer, index);
                let rows = CVPixelBufferGetHeightOfPlane(buffer, index);
                let base = CVPixelBufferGetBaseAddressOfPlane(buffer, index) as *const u8;
                (std::slice::from_raw_parts(base, stride * rows), stride)
            };
            let (luma, luma_stride) = plane(0);
            let (chroma, chroma_stride) = plane(1);
            Self {
                buffer,
                width: CVPixelBufferGetWidthOfPlane(buffer, 0),
                height: CVPixelBufferGetHeightOfPlane(buffer, 0),
                luma,
                luma_stride,
                chroma,
                chroma_stride,
            }
        }
    }

    /// Mean luma over every 8th pixel of every 8th row.
    pub fn mean(&self) -> u8 {
        let (mut sum, mut count) = (0u64, 0u64);
        for y in (0..self.height).step_by(8) {
            let row = &self.luma[y * self.luma_stride..][..self.width];
            for value in row.iter().step_by(8) {
                sum += u64::from(*value);
                count += 1;
            }
        }
        (sum / count.max(1)) as u8
    }

    /// Mean luma of each block of a `columns` x `rows` grid, sampled sparsely.
    pub fn grid(&self, (columns, rows): (usize, usize)) -> Vec<u8> {
        let mut out = Vec::with_capacity(columns * rows);
        for gy in 0..rows {
            for gx in 0..columns {
                let (x0, x1) = (gx * self.width / columns, (gx + 1) * self.width / columns);
                let (y0, y1) = (gy * self.height / rows, (gy + 1) * self.height / rows);
                let (mut sum, mut count) = (0u64, 0u64);
                for y in (y0..y1).step_by(4) {
                    for x in (x0..x1).step_by(4) {
                        sum += u64::from(self.luma[y * self.luma_stride + x]);
                        count += 1;
                    }
                }
                out.push((sum / count.max(1)) as u8);
            }
        }
        out
    }

    /// The frame as 8-bit RGB, BT.709 full range, for writing a still.
    pub fn rgb(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.width * self.height * 3);
        for y in 0..self.height {
            let row = &self.luma[y * self.luma_stride..][..self.width];
            let chroma = &self.chroma[(y / 2) * self.chroma_stride..];
            for (x, luma) in row.iter().enumerate() {
                let l = f32::from(*luma);
                let cb = f32::from(chroma[(x / 2) * 2]) - 128.0;
                let cr = f32::from(chroma[(x / 2) * 2 + 1]) - 128.0;
                let r = l + 1.5748 * cr;
                let g = l - 0.1873 * cb - 0.4681 * cr;
                let b = l + 1.8556 * cb;
                out.extend([r, g, b].map(|v| v.clamp(0.0, 255.0) as u8));
            }
        }
        out
    }
}

impl Drop for Planes<'_> {
    fn drop(&mut self) {
        // SAFETY: locked read-only in `lock` with the same flags.
        unsafe { CVPixelBufferUnlockBaseAddress(self.buffer, CVPixelBufferLockFlags::ReadOnly) };
    }
}

/// Runs every check on the cut named `cut`, measuring its export at `export`. The caption
/// sidecar and thumbnail sit next to the export as `<export>.vtt` and `<export>.png` (or
/// `.jpg`).
pub fn check_cut(
    video_dir: &Path,
    project: &Project,
    cut: &str,
    export: &Path,
) -> Result<Vec<Problem>, MediaError> {
    let timeline = Timeline::evaluate(project);
    let output = timeline
        .outputs
        .iter()
        .find(|o| o.name == cut)
        .ok_or_else(|| MediaError::Video(format!("there is no cut named {cut}")))?;
    let mut transcripts = HashMap::new();
    for segment in output.chapters.iter().flat_map(|c| &c.segments) {
        if transcripts.contains_key(&segment.take) {
            continue;
        }
        if let Some(transcript) = shot_analyze::cache::read_cached(video_dir, &segment.take)
            .map_err(|e| MediaError::Audio(e.to_string()))?
        {
            transcripts.insert(segment.take.clone(), transcript);
        }
    }
    let words = spoken_words(output, &transcripts);
    let length = duration(output);
    let mut problems = Vec::new();
    if export.exists() {
        let (lufs, peak) = loudness(export)?;
        problems.extend(check::loudness(lufs, peak));
        problems.extend(check::frames(&sample_frames(export, length)?));
    } else {
        problems.push(Problem {
            check: CheckKind::Loudness,
            message: format!(
                "no export at {}, so loudness and frames are unchecked: run `shot export --cut {cut}`",
                export.display()
            ),
        });
    }
    let sidecar = std::fs::read_to_string(export.with_extension("vtt")).ok();
    problems.extend(check::captions(
        output.captions,
        output.kind.burns_captions(),
        sidecar.as_deref(),
        &output.caption_fixes,
        &words,
        &check::takes_without_transcript(output, &transcripts),
    ));
    problems.extend(check::links(
        &check::said_links(&words),
        &check::link_cards(output),
    ));
    problems.extend(check::chapters(
        output.kind,
        &chapter_marks(output, &words),
        length,
    ));
    let thumbnail = ["png", "jpg"]
        .iter()
        .any(|ext| export.with_extension(ext).exists());
    problems.extend(check::thumbnail(output.kind, thumbnail));
    problems.sort_by_key(|p| p.check);
    Ok(problems)
}
