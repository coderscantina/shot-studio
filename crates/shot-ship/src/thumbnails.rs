//! Thumbnail candidates: camera frames of the main cut ranked by expression strength.
//!
//! Each sampled frame runs Apple Vision's face capture quality and face landmarks. The score
//! mixes capture quality (sharp, lit, facing the camera) with how open the mouth is, the
//! clearest sign of an expression in a talking-head frame. The best frames at least
//! [`SPACING`] apart are written as PNG into the cache, where a person picks one.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use objc2::AnyThread;
use objc2::rc::{Retained, autoreleasepool};
use objc2_foundation::{NSArray, NSDictionary};
use objc2_vision::{
    VNDetectFaceCaptureQualityRequest, VNDetectFaceLandmarksRequest, VNFaceObservation,
    VNImageRequestHandler, VNRequest,
};
use shot_core::{Id, Layout, Micros, Project, Timeline};
use shot_media::videotoolbox::Decoder;

use crate::media::Planes;

/// Camera frames are scored this often in kept material.
pub const STEP: Micros = Micros(1_000_000);
/// Candidates are at least this far apart, so they are not the same moment twice.
pub const SPACING: Micros = Micros(10_000_000);

/// A scored frame of the camera track.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub take: Id,
    /// Media time in the take.
    pub source: Micros,
    /// Where the frame plays in the main cut.
    pub at: Micros,
    /// 0 to 1, higher is a stronger, cleaner expression.
    pub score: f32,
    /// The written still, once [`find`] wrote it.
    pub path: Option<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum ThumbnailError {
    #[error("{path}: {message}")]
    Decode { path: PathBuf, message: String },
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

/// Scores the camera frames the main cut shows and writes the best `count` as PNG under
/// `.shot/cache/thumbnails/`.
pub fn find(
    video_dir: &Path,
    project: &Project,
    count: usize,
) -> Result<Vec<Candidate>, ThumbnailError> {
    let main = &Timeline::evaluate(project).outputs[0];
    let mut scored = Vec::new();
    let mut offset = 0;
    let mut decoders: Vec<(Id, Option<Decoder>)> = Vec::new();
    for segment in main.chapters.iter().flat_map(|c| &c.segments) {
        let source = segment.source;
        if shows_face(segment.layout) {
            let decoder = match decoders.iter().position(|(take, _)| *take == segment.take) {
                Some(index) => &mut decoders[index].1,
                None => {
                    let path = camera(video_dir, &segment.take);
                    let opened = path.exists().then(|| Decoder::open(&path)).transpose();
                    let opened = opened.map_err(|e| ThumbnailError::Decode {
                        path: path.clone(),
                        message: e.to_string(),
                    })?;
                    decoders.push((segment.take.clone(), opened));
                    &mut decoders.last_mut().expect("just pushed").1
                }
            };
            if let Some(decoder) = decoder {
                let mut t = source.from().0.div_ceil(STEP.0) * STEP.0;
                while t < source.to().0 {
                    let frame = decoder
                        .seek(Duration::from_micros(t), || false)
                        .map_err(|e| ThumbnailError::Decode {
                            path: camera(video_dir, &segment.take),
                            message: e.to_string(),
                        })?;
                    if let Some(score) = frame.as_ref().and_then(|f| expression(&f.buffer)) {
                        scored.push(Candidate {
                            take: segment.take.clone(),
                            source: Micros(t),
                            at: Micros(offset + t - source.from().0),
                            score,
                            path: None,
                        });
                    }
                    t += STEP.0;
                }
            }
        }
        offset += source.duration().0;
    }
    let mut best = rank(scored, count);
    let dir = video_dir.join(".shot/cache/thumbnails");
    let io = |path: &Path| {
        let path = path.to_owned();
        move |source| ThumbnailError::Io { path, source }
    };
    fs::create_dir_all(&dir).map_err(io(&dir))?;
    for candidate in &mut best {
        let path = dir.join(format!(
            "{}-{}.png",
            candidate.take.as_str(),
            candidate.source.0 / 1_000
        ));
        let file = camera(video_dir, &candidate.take);
        let mut decoder = Decoder::open(&file).map_err(|e| ThumbnailError::Decode {
            path: file.clone(),
            message: e.to_string(),
        })?;
        if let Some(frame) = decoder
            .seek(Duration::from_micros(candidate.source.0), || false)
            .map_err(|e| ThumbnailError::Decode {
                path: file.clone(),
                message: e.to_string(),
            })?
        {
            write_png(&Planes::lock(&frame.buffer), &path).map_err(io(&path))?;
            candidate.path = Some(path);
        }
    }
    Ok(best)
}

/// The best `count` candidates, highest score first, at least [`SPACING`] apart in the cut.
pub fn rank(mut scored: Vec<Candidate>, count: usize) -> Vec<Candidate> {
    scored.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut picked: Vec<Candidate> = Vec::new();
    for candidate in scored {
        if picked.len() == count {
            break;
        }
        if picked
            .iter()
            .all(|p| p.at.0.abs_diff(candidate.at.0) >= SPACING.0)
        {
            picked.push(candidate);
        }
    }
    picked
}

fn shows_face(layout: Layout) -> bool {
    matches!(
        layout,
        Layout::Face | Layout::FaceContent | Layout::Split | Layout::FaceCorner
    )
}

fn camera(video_dir: &Path, take: &Id) -> PathBuf {
    video_dir
        .join("media")
        .join(take.as_str())
        .join("camera.mp4")
}

/// The expression score of the largest face in a frame, `None` without a face.
fn expression(buffer: &objc2_core_video::CVPixelBuffer) -> Option<f32> {
    autoreleasepool(|_| {
        // SAFETY: plain initializers, an empty options dictionary is valid, and the buffer is a
        // decoded frame.
        let (quality, landmarks) = unsafe {
            (
                VNDetectFaceCaptureQualityRequest::new(),
                VNDetectFaceLandmarksRequest::new(),
            )
        };
        let handler = unsafe {
            VNImageRequestHandler::initWithCVPixelBuffer_options(
                VNImageRequestHandler::alloc(),
                buffer,
                &NSDictionary::new(),
            )
        };
        let (a, b): (&VNRequest, &VNRequest) = (&quality, &landmarks);
        let requests = NSArray::from_slice(&[a, b]);
        handler.performRequests_error(&requests).ok()?;
        // SAFETY: results are read after the requests ran on this thread.
        let quality = unsafe { quality.results() }
            .and_then(|r| largest(&r))
            .and_then(|face| unsafe { face.faceCaptureQuality() })
            .map(|n| n.floatValue())?;
        let mouth = unsafe { landmarks.results() }
            .and_then(|r| largest(&r))
            .and_then(|face| unsafe { face.landmarks() })
            .and_then(|l| unsafe { l.outerLips() })
            .map(|lips| {
                // SAFETY: Vision returns `pointCount` points that live as long as the region.
                let points = unsafe {
                    std::slice::from_raw_parts(lips.normalizedPoints(), lips.pointCount())
                };
                openness(points.iter().map(|p| (p.x as f32, p.y as f32)))
            })
            .unwrap_or(0.0);
        Some(score(quality, mouth))
    })
}

fn largest(faces: &NSArray<VNFaceObservation>) -> Option<Retained<VNFaceObservation>> {
    faces.iter().max_by(|a, b| {
        // SAFETY: plain geometry getters on a finished observation.
        let area = |f: &VNFaceObservation| {
            let r = unsafe { f.boundingBox() };
            r.size.width * r.size.height
        };
        area(a).total_cmp(&area(b))
    })
}

/// The mouth's height over its width.
fn openness(points: impl Iterator<Item = (f32, f32)>) -> f32 {
    let (mut x0, mut x1, mut y0, mut y1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
    for (x, y) in points {
        (x0, x1, y0, y1) = (x0.min(x), x1.max(x), y0.min(y), y1.max(y));
    }
    if x1 > x0 { (y1 - y0) / (x1 - x0) } else { 0.0 }
}

/// Capture quality weighs more than expression, so a blurred shout never wins. A mouth half as
/// high as wide counts as fully open.
pub fn score(quality: f32, mouth_openness: f32) -> f32 {
    0.6 * quality.clamp(0.0, 1.0) + 0.4 * (mouth_openness / 0.5).clamp(0.0, 1.0)
}

pub(crate) fn write_png(planes: &Planes<'_>, path: &Path) -> std::io::Result<()> {
    let file = std::io::BufWriter::new(fs::File::create(path)?);
    let mut encoder = png::Encoder::new(file, planes.width as u32, planes.height as u32);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(std::io::Error::other)?;
    writer
        .write_image_data(&planes.rgb())
        .map_err(std::io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(at: u64, score: f32) -> Candidate {
        Candidate {
            take: Id::new("t01").unwrap(),
            source: Micros(at),
            at: Micros(at),
            score,
            path: None,
        }
    }

    #[test]
    fn ranks_by_score_and_keeps_candidates_apart() {
        let picked = rank(
            vec![
                candidate(0, 0.5),
                candidate(3_000_000, 0.9),
                candidate(5_000_000, 0.8),
                candidate(20_000_000, 0.7),
            ],
            2,
        );
        let at: Vec<_> = picked.iter().map(|c| c.at.0).collect();
        assert_eq!(at, [3_000_000, 20_000_000]);
    }

    #[test]
    fn expression_rewards_an_open_mouth_on_a_clean_capture() {
        let closed = openness([(0.0, 0.0), (1.0, 0.0), (0.5, 0.1)].into_iter());
        let open = openness([(0.0, 0.0), (1.0, 0.0), (0.5, 0.6)].into_iter());
        assert!(score(0.8, open) > score(0.8, closed));
        assert_eq!(score(1.0, 1.0), 1.0);
    }
}
