//! Person masks of a take's camera, for the camera slot's cut-out (decision 0034).
//!
//! Every frame of `media/<take>/camera.mp4`, at most one each [`STEP`], runs Vision's person segmentation at the balanced
//! quality level, in a sequence handler so the mask stays steady from frame to frame. The mask
//! is stored [`WIDTH`] pixels wide at the camera's aspect, 0 for background to 255 for the
//! person, with a confidence per frame: Vision's own, or 0 when the person covers less than
//! [`MIN_COVER`] of the frame. [`PersonMasks::cutout`] falls back to the plain camera around a
//! frame that is not [`CONFIDENT`], crossfading over [`FADE`].
//!
//! Derived data in `.shot/cache/masks/<take>.bin`, safe to delete. Like the files of
//! [`crate::vision`] it remembers the size and modification time of the track it was read
//! from, so a recording made again is read again. Little endian:
//!
//! ```text
//! "SHOTMASK" version:u32 track_len:u64 track_mtime_ns:u128 width:u32 height:u32 frames:u32
//! frames x { t_us:u64 confidence:u8 bytes:u32 run-length pairs (count:u8 1..=255, value:u8) }
//! ```

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use shot_core::{Id, Micros};

use crate::vision::VisionError;

const MAGIC: &[u8; 8] = b"SHOTMASK";
const VERSION: u32 = 1;
/// The stored mask's width in pixels. The render graph scales it to the camera slot with
/// bilinear filtering, which is the feather of the cut-out's edge.
pub const WIDTH: u32 = 256;
/// The least share of the frame a person covers for the mask to count.
pub const MIN_COVER: f32 = 0.02;
/// The least confidence, of 255, a frame's mask needs to be drawn.
pub const CONFIDENT: u8 = 128;
/// How long the cut-out takes to fade into the plain camera around a frame without a mask.
pub const FADE: Micros = Micros(200_000);
/// Masks are at least this far apart, about 30 a second: a 60 fps camera's frame shows the
/// mask of the frame before it, at half the cost.
pub const STEP: Micros = Micros(33_000);

/// The cache file of `take`'s masks, relative to the video folder.
pub fn cache_file(take: &Id) -> String {
    format!(".shot/cache/masks/{}.bin", take.as_str())
}

/// One camera frame's mask, run-length encoded.
#[derive(Debug, Clone, PartialEq)]
struct Frame {
    t: Micros,
    confidence: u8,
    runs: Box<[u8]>,
}

/// The masks of every frame of a take's camera.
#[derive(Debug, Clone, PartialEq)]
pub struct PersonMasks {
    pub width: u32,
    pub height: u32,
    /// In presentation order.
    frames: Vec<Frame>,
}

/// What the camera slot draws at one moment: the mask of the frame on screen and how far the
/// cut-out is faded in over the plain camera.
#[derive(Debug, Clone, PartialEq)]
pub struct Cutout {
    pub width: u32,
    pub height: u32,
    /// `width * height` bytes, row by row from the top, 0 for background to 255 for the person.
    pub alpha: Arc<[u8]>,
    /// 0 draws the plain camera, 1 the person alone.
    pub weight: f32,
}

impl PersonMasks {
    /// The masks of `take` if they were computed for its camera track as it is now.
    pub fn read(video: &Path, take: &Id) -> Option<Self> {
        let track = camera(video, take);
        let identity = crate::vision::identity(&track).ok()?;
        let bytes = fs::read(video.join(cache_file(take))).ok()?;
        let (read_identity, masks) = decode(&bytes)?;
        (read_identity == identity).then_some(masks)
    }

    /// Masks of `width` by `height` from `(time, confidence, alpha)` frames in presentation
    /// order, each alpha `width * height` bytes.
    pub fn new(
        width: u32,
        height: u32,
        frames: impl IntoIterator<Item = (Micros, u8, Vec<u8>)>,
    ) -> Self {
        let frames = frames
            .into_iter()
            .map(|(t, confidence, alpha)| {
                assert_eq!(
                    alpha.len(),
                    (width * height) as usize,
                    "a mask is width * height"
                );
                Frame {
                    t,
                    confidence,
                    runs: run(&alpha),
                }
            })
            .collect();
        Self {
            width,
            height,
            frames,
        }
    }

    /// Writes the masks as the cache of `take`'s camera track as it is now.
    pub fn write(&self, video: &Path, take: &Id) -> Result<(), VisionError> {
        let identity = crate::vision::identity(&camera(video, take))?;
        let path = video.join(cache_file(take));
        let io_error = |path: &Path| {
            let path = path.to_owned();
            move |source| VisionError::Io { path, source }
        };
        let dir = path.parent().expect("cache path has a parent");
        fs::create_dir_all(dir).map_err(io_error(dir))?;
        let tmp = path.with_extension("bin.tmp");
        fs::write(&tmp, encode(identity, self)).map_err(io_error(&tmp))?;
        fs::rename(&tmp, &path).map_err(io_error(&path))
    }

    /// The cut-out on screen at media time `t`, `None` where the plain camera shows.
    pub fn cutout(&self, t: Micros) -> Option<Cutout> {
        let index = self.frames.partition_point(|f| f.t <= t).checked_sub(1)?;
        let from = Micros(t.0.saturating_sub(FADE.0));
        let start = self.frames.partition_point(|f| f.t < from);
        let mut weight = 1.0f32;
        for frame in &self.frames[start..] {
            if frame.t.0 > t.0 + FADE.0 {
                break;
            }
            if frame.confidence < CONFIDENT {
                weight = weight.min(frame.t.0.abs_diff(t.0) as f32 / FADE.0 as f32);
            }
        }
        if weight <= 0.0 {
            return None;
        }
        Some(Cutout {
            width: self.width,
            height: self.height,
            alpha: unrun(
                &self.frames[index].runs,
                (self.width * self.height) as usize,
            )
            .into(),
            weight,
        })
    }

    /// How many frames hold a mask confident enough to draw, of all frames.
    pub fn confident(&self) -> (usize, usize) {
        let good = self.frames.iter().filter(|f| f.confidence >= CONFIDENT);
        (good.count(), self.frames.len())
    }
}

/// The masks of `take`'s camera, read from the cache or computed and cached. `progress` hears
/// the media time of each computed frame.
pub fn person_masks(
    video: &Path,
    take: &Id,
    progress: impl FnMut(Micros),
) -> Result<PersonMasks, VisionError> {
    if let Some(masks) = PersonMasks::read(video, take) {
        return Ok(masks);
    }
    let masks = platform::compute(&camera(video, take), progress)?;
    masks.write(video, take)?;
    Ok(masks)
}

fn camera(video: &Path, take: &Id) -> PathBuf {
    video.join("media").join(take.as_str()).join("camera.mp4")
}

/// Run-length pairs of `pixels`.
fn run(pixels: &[u8]) -> Box<[u8]> {
    let mut out = Vec::new();
    let mut rest = pixels;
    while let Some(&value) = rest.first() {
        let count = rest.iter().take(255).take_while(|&&v| v == value).count();
        out.extend_from_slice(&[count as u8, value]);
        rest = &rest[count..];
    }
    out.into_boxed_slice()
}

/// The `len` pixels of run-length pairs, zeros past their end.
fn unrun(runs: &[u8], len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    for &[count, value] in runs.as_chunks::<2>().0 {
        out.extend(std::iter::repeat_n(value, usize::from(count)));
    }
    out.resize(len, 0);
    out
}

/// `src` of `src_size` box-averaged down (or sampled up) to `size`.
fn resample(src: &[u8], src_size: (u32, u32), size: (u32, u32)) -> Vec<u8> {
    let (sw, sh) = (src_size.0 as usize, src_size.1 as usize);
    let (w, h) = (size.0 as usize, size.1 as usize);
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        let (y0, y1) = (y * sh / h, ((y + 1) * sh / h).max(y * sh / h + 1));
        for x in 0..w {
            let (x0, x1) = (x * sw / w, ((x + 1) * sw / w).max(x * sw / w + 1));
            let mut sum = 0u32;
            for row in src[y0 * sw..y1 * sw].chunks_exact(sw) {
                sum += row[x0..x1].iter().map(|&v| u32::from(v)).sum::<u32>();
            }
            out.push((sum / ((x1 - x0) * (y1 - y0)) as u32) as u8);
        }
    }
    out
}

/// One frame's stored mask from Vision's: scaled to `size`, its confidence 0 when the person
/// covers too little of it.
fn frame(t: Micros, vision: Option<(&[u8], (u32, u32), f32)>, size: (u32, u32)) -> Frame {
    let len = (size.0 * size.1) as usize;
    let Some((pixels, vision_size, confidence)) = vision else {
        return Frame {
            t,
            confidence: 0,
            runs: run(&vec![0; len]),
        };
    };
    let mask = resample(pixels, vision_size, size);
    let covered = mask.iter().filter(|&&v| v > 127).count();
    let confidence = if (covered as f32) < MIN_COVER * len as f32 {
        0
    } else {
        (confidence.clamp(0.0, 1.0) * 255.0).round() as u8
    };
    Frame {
        t,
        confidence,
        runs: run(&mask),
    }
}

/// The stored mask size of a camera frame of `width` by `height`.
fn mask_size(width: u32, height: u32) -> (u32, u32) {
    let h = (u64::from(WIDTH) * u64::from(height) / u64::from(width.max(1))).max(1);
    (WIDTH, h as u32)
}

fn encode(identity: (u64, u128), masks: &PersonMasks) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&identity.0.to_le_bytes());
    out.extend_from_slice(&identity.1.to_le_bytes());
    out.extend_from_slice(&masks.width.to_le_bytes());
    out.extend_from_slice(&masks.height.to_le_bytes());
    out.extend_from_slice(&(masks.frames.len() as u32).to_le_bytes());
    for frame in &masks.frames {
        out.extend_from_slice(&frame.t.0.to_le_bytes());
        out.push(frame.confidence);
        out.extend_from_slice(&(frame.runs.len() as u32).to_le_bytes());
        out.extend_from_slice(&frame.runs);
    }
    out
}

/// The track identity and masks of a cache file, `None` for another version or a broken file.
fn decode(bytes: &[u8]) -> Option<((u64, u128), PersonMasks)> {
    let mut rest = bytes;
    let mut take = |n: usize| {
        let (head, tail) = rest.split_at_checked(n)?;
        rest = tail;
        Some(head)
    };
    if take(8)? != MAGIC || u32::from_le_bytes(take(4)?.try_into().ok()?) != VERSION {
        return None;
    }
    let len = u64::from_le_bytes(take(8)?.try_into().ok()?);
    let modified = u128::from_le_bytes(take(16)?.try_into().ok()?);
    let width = u32::from_le_bytes(take(4)?.try_into().ok()?);
    let height = u32::from_le_bytes(take(4)?.try_into().ok()?);
    let count = u32::from_le_bytes(take(4)?.try_into().ok()?);
    let mut frames = Vec::with_capacity(count.min(1 << 20) as usize);
    for _ in 0..count {
        let t = Micros(u64::from_le_bytes(take(8)?.try_into().ok()?));
        let confidence = take(1)?[0];
        let bytes = u32::from_le_bytes(take(4)?.try_into().ok()?) as usize;
        frames.push(Frame {
            t,
            confidence,
            runs: take(bytes)?.into(),
        });
    }
    Some((
        (len, modified),
        PersonMasks {
            width,
            height,
            frames,
        },
    ))
}

#[cfg(target_os = "macos")]
pub use platform::PersonSegmenter;

#[cfg(target_os = "macos")]
mod platform {
    use std::path::Path;

    use objc2::rc::{Retained, autoreleasepool};
    use objc2_core_video::{
        CVPixelBuffer, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow,
        CVPixelBufferGetHeight, CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress,
        CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress, kCVPixelFormatType_OneComponent8,
    };
    use objc2_foundation::NSArray;
    use objc2_vision::{
        VNGeneratePersonSegmentationRequest, VNGeneratePersonSegmentationRequestQualityLevel,
        VNRequest, VNSequenceRequestHandler,
    };
    use shot_core::Micros;
    use shot_media::videotoolbox::Decoder;

    use super::{PersonMasks, VisionError, frame, mask_size};

    /// Vision's person segmentation, one frame after another of the same video.
    pub struct PersonSegmenter {
        request: Retained<VNGeneratePersonSegmentationRequest>,
        handler: Retained<VNSequenceRequestHandler>,
    }

    /// A mask as Vision returns it: its pixels, its size and Vision's confidence.
    pub type VisionMask = (Vec<u8>, (u32, u32), f32);

    impl PersonSegmenter {
        /// A segmenter at `quality`: balanced for video, accurate for a still.
        pub fn new(quality: VNGeneratePersonSegmentationRequestQualityLevel) -> Self {
            // SAFETY: plain initializers and setters on objects this thread owns.
            unsafe {
                let request = VNGeneratePersonSegmentationRequest::new();
                request.setQualityLevel(quality);
                request.setOutputPixelFormat(kCVPixelFormatType_OneComponent8);
                Self {
                    request,
                    handler: VNSequenceRequestHandler::new(),
                }
            }
        }

        /// The person in `buffer`, 0 for background to 255 for the person, `None` when Vision
        /// returns no mask.
        pub fn mask(&self, buffer: &CVPixelBuffer) -> Result<Option<VisionMask>, VisionError> {
            autoreleasepool(|_| {
                let request: &VNRequest = &self.request;
                // SAFETY: the buffer is a decoded frame and the request is this segmenter's.
                unsafe {
                    self.handler
                        .performRequests_onCVPixelBuffer_error(
                            &NSArray::from_slice(&[request]),
                            buffer,
                        )
                        .map_err(|e| VisionError::Request(e.localizedDescription().to_string()))?;
                }
                // SAFETY: read after the request ran on this thread.
                let Some(observation) =
                    unsafe { self.request.results() }.and_then(|r| r.firstObject())
                else {
                    return Ok(None);
                };
                let confidence = unsafe { observation.confidence() };
                let mask = unsafe { observation.pixelBuffer() };
                // SAFETY: a one-component 8-bit buffer, locked read-only while its rows are
                // copied.
                unsafe {
                    CVPixelBufferLockBaseAddress(&mask, CVPixelBufferLockFlags::ReadOnly);
                    let (width, height) =
                        (CVPixelBufferGetWidth(&mask), CVPixelBufferGetHeight(&mask));
                    let stride = CVPixelBufferGetBytesPerRow(&mask);
                    let base = CVPixelBufferGetBaseAddress(&mask) as *const u8;
                    let rows = std::slice::from_raw_parts(base, stride * height);
                    let pixels: Vec<u8> = rows
                        .chunks(stride)
                        .flat_map(|row| &row[..width])
                        .copied()
                        .collect();
                    CVPixelBufferUnlockBaseAddress(&mask, CVPixelBufferLockFlags::ReadOnly);
                    Ok(Some((pixels, (width as u32, height as u32), confidence)))
                }
            })
        }
    }

    pub fn compute(
        path: &Path,
        mut progress: impl FnMut(Micros),
    ) -> Result<PersonMasks, VisionError> {
        let decode = |message: String| VisionError::Decode {
            path: path.to_owned(),
            message,
        };
        let mut decoder = Decoder::open(path).map_err(|e| decode(e.to_string()))?;
        let segmenter =
            PersonSegmenter::new(VNGeneratePersonSegmentationRequestQualityLevel::Balanced);
        let mut size = None;
        let mut frames = Vec::new();
        while let Some(decoded) = decoder.next_frame().map_err(|e| decode(e.to_string()))? {
            let t = Micros(decoded.pts.as_micros() as u64);
            let size = *size.get_or_insert_with(|| {
                mask_size(
                    CVPixelBufferGetWidth(&decoded.buffer) as u32,
                    CVPixelBufferGetHeight(&decoded.buffer) as u32,
                )
            });
            if frames
                .last()
                .is_some_and(|f: &super::Frame| t.0 < f.t.0 + super::STEP.0)
            {
                continue;
            }
            let mask = segmenter.mask(&decoded.buffer)?;
            let vision = mask.as_ref().map(|(p, s, c)| (p.as_slice(), *s, *c));
            frames.push(frame(t, vision, size));
            progress(t);
        }
        let (width, height) = size.unwrap_or_else(|| mask_size(16, 9));
        Ok(PersonMasks {
            width,
            height,
            frames,
        })
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use std::path::Path;

    use shot_core::Micros;

    use super::{PersonMasks, VisionError};

    pub fn compute(_: &Path, _: impl FnMut(Micros)) -> Result<PersonMasks, VisionError> {
        Err(VisionError::Request("Vision needs macOS".to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn masks(confidences: &[u8]) -> PersonMasks {
        let size = (4, 2);
        PersonMasks {
            width: size.0,
            height: size.1,
            frames: confidences
                .iter()
                .enumerate()
                .map(|(i, &c)| Frame {
                    t: Micros(i as u64 * 50_000),
                    confidence: c,
                    runs: run(&[255, 255, 0, 0, 255, 255, 0, 0]),
                })
                .collect(),
        }
    }

    #[test]
    fn masks_survive_the_cache_file() {
        let masks = masks(&[255, 0, 200]);
        let bytes = encode((7, 9), &masks);
        assert_eq!(decode(&bytes), Some(((7, 9), masks)));
        assert_eq!(decode(&bytes[..bytes.len() - 1]), None);
    }

    #[test]
    fn runs_hold_long_and_short_stretches() {
        let mut pixels = vec![0u8; 600];
        pixels[300..310].copy_from_slice(&[1, 2, 3, 4, 5, 255, 255, 255, 255, 9]);
        assert_eq!(unrun(&run(&pixels), pixels.len()), pixels);
    }

    #[test]
    fn a_frame_without_a_person_fades_to_the_plain_camera() {
        // Frames every 50 ms; the one at 500 ms holds no person.
        let mut confidences = vec![255u8; 21];
        confidences[10] = 0;
        let masks = masks(&confidences);
        assert_eq!(masks.cutout(Micros(500_000)), None);
        let weight = |t| masks.cutout(Micros(t)).map_or(0.0, |c| c.weight);
        assert!((weight(400_000) - 0.5).abs() < 1e-6);
        assert!((weight(250_000) - 1.0).abs() < 1e-6);
        let cut = masks.cutout(Micros(0)).unwrap();
        assert_eq!(&*cut.alpha, &[255, 255, 0, 0, 255, 255, 0, 0]);
    }

    #[test]
    fn a_small_person_counts_as_none() {
        let tiny = [0u8; 100];
        let mut person = [0u8; 100];
        person[..40].fill(255);
        assert_eq!(
            frame(Micros(0), Some((&tiny, (10, 10), 1.0)), (5, 5)).confidence,
            0
        );
        assert_eq!(
            frame(Micros(0), Some((&person, (10, 10), 1.0)), (5, 5)).confidence,
            255
        );
        assert_eq!(frame(Micros(0), None, (5, 5)).confidence, 0);
    }
}
