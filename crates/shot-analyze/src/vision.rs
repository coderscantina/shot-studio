//! Faces and on-screen text with Apple Vision, sampled from a take's video tracks and cached.
//!
//! - [`face_track`]: the largest face of `camera.mp4` at given times, for reframing derived cuts.
//! - [`screen_text`]: the text lines of `screen.mp4` at given times, for highlights and blur.
//!
//! Both are derived data under `.shot/cache/` (`faces/<take>.json`, `text/<take>.json`), safe to
//! delete. A cache file remembers the size and modification time of the track it was read from
//! and the times it holds, so a recording made again is read again and a new time costs one
//! decode and one Vision request:
//!
//! ```json
//! { "version": 1, "track": [5326198, 1762500000000000000], "size": [1920, 1080], "samples": [[0, {"x": 0.4, "y": 0.2, "w": 0.2, "h": 0.36}]] }
//! ```
//!
//! Rectangles are normalized to the frame, origin top left, like the screen coordinates of the
//! event log.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use shot_core::{Id, Micros};

const VERSION: u32 = 1;

/// A rectangle in fractions of the frame, origin top left.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NormRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl NormRect {
    pub fn center(self) -> [f32; 2] {
        [self.x + self.w / 2.0, self.y + self.h / 2.0]
    }

    pub fn area(self) -> f32 {
        self.w * self.h
    }
}

/// One line of recognized text and where it is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextLine {
    pub text: String,
    pub rect: NormRect,
}

impl TextLine {
    /// The part of the line that shows `chars`, a byte range of `text`, assuming evenly wide
    /// characters. Vision boxes whole lines; code and UI text are close enough to even.
    pub fn span(&self, chars: std::ops::Range<usize>) -> NormRect {
        let count = self.text.chars().count().max(1) as f32;
        let start = self.text[..chars.start].chars().count() as f32;
        let end = self.text[..chars.end].chars().count() as f32;
        NormRect {
            x: self.rect.x + self.rect.w * start / count,
            y: self.rect.y,
            w: self.rect.w * (end - start) / count,
            h: self.rect.h,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum VisionError {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("{path}: {message}")]
    Decode { path: PathBuf, message: String },
    #[error("Vision failed: {0}")]
    Request(String),
}

/// What a cache file holds: one value per sampled time.
#[derive(Serialize, Deserialize)]
struct CacheFile<T> {
    version: u32,
    /// Size and modification time in nanoseconds of the track the samples come from.
    track: (u64, u128),
    /// The track's frame size in pixels.
    size: (u32, u32),
    samples: Vec<(u64, T)>,
}

/// The frame size and the values read at each time, as the platform returns them.
type Frames<T> = ((u32, u32), Vec<(Micros, T)>);

/// Values read from a track's frames, with the frame size.
#[derive(Debug, Clone, PartialEq)]
pub struct Sampled<T> {
    pub size: (u32, u32),
    /// In the order of the times asked for.
    pub samples: Vec<(Micros, T)>,
}

/// The largest face in `media/<take>/camera.mp4` at each of `times`, `None` where Vision sees
/// none.
pub fn face_track(
    video: &Path,
    take: &Id,
    times: &[Micros],
) -> Result<Sampled<Option<NormRect>>, VisionError> {
    sampled(video, take, "camera.mp4", "faces", times, |buffer| {
        Ok(platform::faces(buffer)?
            .into_iter()
            .max_by(|a, b| a.area().total_cmp(&b.area())))
    })
}

/// The text lines of `media/<take>/screen.mp4` at each of `times`.
pub fn screen_text(
    video: &Path,
    take: &Id,
    times: &[Micros],
) -> Result<Sampled<Vec<TextLine>>, VisionError> {
    sampled(video, take, "screen.mp4", "text", times, platform::text)
}

/// Samples `read` at `times` on a track of a take, through the cache in `.shot/cache/<kind>/`.
fn sampled<T: Clone + Serialize + for<'de> Deserialize<'de>>(
    video: &Path,
    take: &Id,
    track: &str,
    kind: &str,
    times: &[Micros],
    read: impl Fn(&platform::Buffer) -> Result<T, VisionError>,
) -> Result<Sampled<T>, VisionError> {
    let track_path = video.join("media").join(take.as_str()).join(track);
    let identity = identity(&track_path)?;
    let cache_path = video
        .join(".shot/cache")
        .join(kind)
        .join(format!("{}.json", take.as_str()));
    let cached = fs::read(&cache_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<CacheFile<T>>(&bytes).ok())
        .filter(|file| file.version == VERSION && file.track == identity);
    let mut size = cached.as_ref().map(|file| file.size);
    let mut samples: BTreeMap<u64, T> = cached
        .map(|file| file.samples.into_iter().collect())
        .unwrap_or_default();
    let mut missing: Vec<Micros> = times
        .iter()
        .copied()
        .filter(|t| !samples.contains_key(&t.0))
        .collect();
    missing.sort_unstable();
    missing.dedup();
    if !missing.is_empty() || size.is_none() {
        let (read_size, read) = platform::sample(&track_path, &missing, &read)?;
        size = Some(read_size);
        for (t, value) in read {
            samples.insert(t.0, value);
        }
        let file = CacheFile {
            version: VERSION,
            track: identity,
            size: read_size,
            samples: samples.iter().map(|(t, v)| (*t, v.clone())).collect(),
        };
        let io_error = |path: &Path| {
            let path = path.to_owned();
            move |source| VisionError::Io { path, source }
        };
        let dir = cache_path.parent().expect("cache path has a parent");
        fs::create_dir_all(dir).map_err(io_error(dir))?;
        let tmp = cache_path.with_extension("json.tmp");
        let json = serde_json::to_vec(&file).expect("samples serialize");
        fs::write(&tmp, json).map_err(io_error(&tmp))?;
        fs::rename(&tmp, &cache_path).map_err(io_error(&cache_path))?;
    }
    Ok(Sampled {
        size: size.expect("read above"),
        samples: times.iter().map(|t| (*t, samples[&t.0].clone())).collect(),
    })
}

fn identity(path: &Path) -> Result<(u64, u128), VisionError> {
    let metadata = fs::metadata(path).map_err(|source| VisionError::Io {
        path: path.to_owned(),
        source,
    })?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .unwrap_or_default();
    Ok((metadata.len(), modified.as_nanos()))
}

#[cfg(target_os = "macos")]
mod platform {
    use std::path::Path;
    use std::time::Duration;

    use objc2::AnyThread;
    use objc2::rc::{Retained, autoreleasepool};
    use objc2_core_video::{CVPixelBufferGetHeight, CVPixelBufferGetWidth};
    use objc2_foundation::{NSArray, NSDictionary};
    use objc2_vision::{
        VNDetectFaceRectanglesRequest, VNImageRequestHandler, VNRecognizeTextRequest, VNRequest,
        VNRequestTextRecognitionLevel,
    };
    use shot_core::Micros;
    use shot_media::videotoolbox::Decoder;

    use super::{NormRect, TextLine, VisionError};

    pub type Buffer = objc2_core_video::CVPixelBuffer;

    /// Decodes the frame on screen at each of `times` and reads it. Returns the frame size too,
    /// from the first frame.
    pub fn sample<T>(
        path: &Path,
        times: &[Micros],
        read: &impl Fn(&Buffer) -> Result<T, VisionError>,
    ) -> Result<super::Frames<T>, VisionError> {
        let decode = |message: String| VisionError::Decode {
            path: path.to_owned(),
            message,
        };
        let mut decoder = Decoder::open(path).map_err(|e| decode(e.to_string()))?;
        let mut at = |t: Micros| {
            decoder
                .seek(Duration::from_micros(t.0), || false)
                .map_err(|e| decode(e.to_string()))?
                .ok_or_else(|| decode(format!("no frame at {} us", t.0)))
        };
        let first = at(times.first().copied().unwrap_or(Micros(0)))?;
        let size = (
            CVPixelBufferGetWidth(&first.buffer) as u32,
            CVPixelBufferGetHeight(&first.buffer) as u32,
        );
        let mut out = Vec::with_capacity(times.len());
        for &t in times {
            let frame = at(t)?;
            out.push((t, read(&frame.buffer)?));
        }
        Ok((size, out))
    }

    fn perform(buffer: &Buffer, request: &VNRequest) -> Result<(), VisionError> {
        let handler = unsafe {
            VNImageRequestHandler::initWithCVPixelBuffer_options(
                VNImageRequestHandler::alloc(),
                buffer,
                &NSDictionary::new(),
            )
        };
        handler
            .performRequests_error(&NSArray::from_slice(&[request]))
            .map_err(|e| VisionError::Request(e.localizedDescription().to_string()))
    }

    /// Vision's boxes have their origin at the bottom left.
    fn flipped(rect: objc2_core_foundation::CGRect) -> NormRect {
        NormRect {
            x: rect.origin.x as f32,
            y: (1.0 - rect.origin.y - rect.size.height) as f32,
            w: rect.size.width as f32,
            h: rect.size.height as f32,
        }
    }

    pub fn faces(buffer: &Buffer) -> Result<Vec<NormRect>, VisionError> {
        autoreleasepool(|_| {
            let request = unsafe { VNDetectFaceRectanglesRequest::new() };
            perform(buffer, &request)?;
            let results = unsafe { request.results() }.unwrap_or_default();
            Ok(results
                .iter()
                .map(|face| flipped(unsafe { face.boundingBox() }))
                .collect())
        })
    }

    pub fn text(buffer: &Buffer) -> Result<Vec<TextLine>, VisionError> {
        autoreleasepool(|_| {
            let request = VNRecognizeTextRequest::new();
            request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
            request.setUsesLanguageCorrection(false);
            perform(buffer, &request)?;
            let results = request.results().unwrap_or_default();
            Ok(results
                .iter()
                .filter_map(|line| {
                    let best: Retained<_> = line.topCandidates(1).firstObject()?;
                    Some(TextLine {
                        text: best.string().to_string(),
                        rect: flipped(unsafe { line.boundingBox() }),
                    })
                })
                .collect())
        })
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use std::path::Path;

    use shot_core::Micros;

    use super::{NormRect, TextLine, VisionError};

    pub enum Buffer {}

    pub fn sample<T>(
        _: &Path,
        _: &[Micros],
        _: &impl Fn(&Buffer) -> Result<T, VisionError>,
    ) -> Result<super::Frames<T>, VisionError> {
        Err(VisionError::Request("Vision needs macOS".to_owned()))
    }

    pub fn faces(buffer: &Buffer) -> Result<Vec<NormRect>, VisionError> {
        match *buffer {}
    }

    pub fn text(buffer: &Buffer) -> Result<Vec<TextLine>, VisionError> {
        match *buffer {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_span_of_a_line_is_its_share_of_the_box() {
        let line = TextLine {
            text: "never deploy on fridays".to_owned(),
            rect: NormRect {
                x: 0.1,
                y: 0.5,
                w: 0.46,
                h: 0.02,
            },
        };
        let start = line.text.find("deploy").unwrap();
        let span = line.span(start..start + "deploy".len());
        assert!((span.x - 0.22).abs() < 1e-6, "{span:?}");
        assert!((span.w - 0.12).abs() < 1e-6, "{span:?}");
    }
}
