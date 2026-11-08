//! Thumbnail candidates: camera frames of the main cut ranked by expression strength, and the
//! pictures the thumbnail studio designs with.
//!
//! Each sampled frame runs Apple Vision's face capture quality and face landmarks. The score
//! mixes capture quality (sharp, lit, facing the camera) with how open the mouth is, the
//! clearest sign of an expression in a talking-head frame. The best frames at least
//! [`SPACING`] apart are written as PNG into the cache, where a person picks one.
//!
//! For the studio, [`sources`] decodes the screen and camera at one moment and cuts the person
//! out of the camera frame with Vision's person segmentation, and [`write_upload`] makes a
//! rendered variant the `main.png` Ship uploads.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use objc2::AnyThread;
use objc2::rc::{Retained, autoreleasepool};
use objc2_core_video::CVPixelBuffer;
use objc2_foundation::{NSArray, NSDictionary};
use objc2_vision::{
    VNDetectFaceCaptureQualityRequest, VNDetectFaceLandmarksRequest, VNFaceObservation,
    VNGeneratePersonSegmentationRequestQualityLevel, VNImageRequestHandler, VNRequest,
};
use shot_analyze::masks::PersonSegmenter;
use shot_core::{Id, Layout, Micros, Project, TimeRange};
use shot_media::videotoolbox::Decoder;
use shot_render::layout::Rect;
use shot_render::thumbnail::{Cutout, OUTLINE, Picture, Sources, silhouette};

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
    #[error(transparent)]
    Lock(#[from] crate::lock::LockError),
    #[error("stopped")]
    Cancelled,
}

/// Scores the camera frames the main cut shows and writes the best `count` as PNG under
/// `.shot/cache/thumbnails/`.
pub fn find(
    video_dir: &Path,
    project: &Project,
    count: usize,
) -> Result<Vec<Candidate>, ThumbnailError> {
    find_with(video_dir, project, count, &|_| {}, &|| false)
}

/// How far [`find_with`] got: frames scored of the frames to score.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scored {
    pub done: usize,
    pub total: usize,
}

/// [`find`], reporting each scored frame to `progress` and stopping with
/// [`ThumbnailError::Cancelled`] once `cancelled` returns true. The stills of an earlier search
/// are replaced, named by rank so the best sorts first.
pub fn find_with(
    video_dir: &Path,
    project: &Project,
    count: usize,
    progress: &dyn Fn(Scored),
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<Candidate>, ThumbnailError> {
    let timeline = crate::lock::timeline(video_dir, project)?;
    // Every frame to score: its take, media time and time in the cut.
    let mut frames: Vec<(&Id, Micros, Micros)> = Vec::new();
    let mut offset = 0;
    for segment in timeline.main().chapters.iter().flat_map(|c| &c.segments) {
        let source = segment.source;
        if shows_face(segment.layout) && camera(video_dir, &segment.take).exists() {
            let mut t = source.from().0.div_ceil(STEP.0) * STEP.0;
            while t < source.to().0 {
                frames.push((
                    &segment.take,
                    Micros(t),
                    Micros(offset + t - source.from().0),
                ));
                t += STEP.0;
            }
        }
        offset += source.duration().0;
    }
    let total = frames.len();
    let mut scored = Vec::new();
    let mut decoders: Vec<(&Id, Decoder)> = Vec::new();
    for (done, (take, source, at)) in frames.into_iter().enumerate() {
        if cancelled() {
            return Err(ThumbnailError::Cancelled);
        }
        let decoder = match decoders.iter().position(|(t, _)| *t == take) {
            Some(index) => &mut decoders[index].1,
            None => {
                let path = camera(video_dir, take);
                let opened = Decoder::open(&path).map_err(|e| ThumbnailError::Decode {
                    path: path.clone(),
                    message: e.to_string(),
                })?;
                decoders.push((take, opened));
                &mut decoders.last_mut().expect("just pushed").1
            }
        };
        let frame = decoder
            .seek(Duration::from_micros(source.0), || false)
            .map_err(|e| ThumbnailError::Decode {
                path: camera(video_dir, take),
                message: e.to_string(),
            })?;
        if let Some(score) = frame.as_ref().and_then(|f| expression(&f.buffer)) {
            scored.push(Candidate {
                take: take.clone(),
                source,
                at,
                score,
                path: None,
            });
        }
        progress(Scored {
            done: done + 1,
            total,
        });
    }
    let mut best = rank(scored, count);
    let dir = video_dir.join(".shot/cache/thumbnails");
    let io = |path: &Path| {
        let path = path.to_owned();
        move |source| ThumbnailError::Io { path, source }
    };
    fs::create_dir_all(&dir).map_err(io(&dir))?;
    for entry in fs::read_dir(&dir).map_err(io(&dir))? {
        let path = entry.map_err(io(&dir))?.path();
        if path.extension().is_some_and(|e| e == "png") {
            fs::remove_file(&path).map_err(io(&path))?;
        }
    }
    for (rank, candidate) in best.iter_mut().enumerate() {
        let path = dir.join(format!(
            "{}-{}-{}.png",
            rank + 1,
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

/// The width the studio's pictures are kept at: the canvas's, enough for a 1280x720 thumbnail.
const STUDIO_WIDTH: u32 = 1280;

/// The main cut in order: each segment's take and the media time it plays, for scrubbing to any
/// moment of the cut.
pub fn main_segments(
    video_dir: &Path,
    project: &Project,
) -> Result<Vec<(Id, TimeRange)>, ThumbnailError> {
    let timeline = crate::lock::timeline(video_dir, project)?;
    Ok(timeline
        .main()
        .chapters
        .iter()
        .flat_map(|c| &c.segments)
        .map(|s| (s.take.clone(), s.source))
        .collect())
}

/// The take and media time that plays at `at` in the cut of `segments`, the last moment past
/// its end.
pub fn moment(segments: &[(Id, TimeRange)], at: Micros) -> Option<(Id, Micros)> {
    let mut offset = 0;
    for (take, source) in segments {
        let length = source.duration().0;
        if at.0 < offset + length {
            return Some((take.clone(), Micros(source.from().0 + at.0 - offset)));
        }
        offset += length;
    }
    let (take, source) = segments.last()?;
    Some((take.clone(), Micros(source.to().0.saturating_sub(1))))
}

/// Where media time `at` of `take` plays in the cut of `segments`, if the cut keeps it.
pub fn cut_time(segments: &[(Id, TimeRange)], take: &Id, at: Micros) -> Option<Micros> {
    let mut offset = 0;
    for (t, source) in segments {
        if t == take && source.from() <= at && at < source.to() {
            return Some(Micros(offset + at.0 - source.from().0));
        }
        offset += source.duration().0;
    }
    None
}

/// The take and media time of a candidate still, from its name `<rank>-<take>-<ms>.png`.
pub fn candidate_moment(still: &Path) -> Option<(Id, Micros)> {
    let stem = still.file_stem()?.to_str()?;
    let mut parts = stem.splitn(3, '-');
    let (_, take, ms) = (parts.next()?, parts.next()?, parts.next()?);
    Some((Id::new(take)?, Micros(ms.parse::<u64>().ok()? * 1_000)))
}

/// The screen and camera of `take` at media time `at`, and the person cut out of the camera.
/// A track the take does not have is `None`, and so is the cutout of a frame without a person.
pub fn sources(video_dir: &Path, take: &Id, at: Micros) -> Result<Sources, ThumbnailError> {
    let track = |file: &str| video_dir.join("media").join(take.as_str()).join(file);
    let screen = decoded(&track("screen.mp4"), at)?.map(|frame| picture(&frame));
    let Some(camera) = decoded(&track("camera.mp4"), at)? else {
        return Ok(Sources {
            screen,
            ..Sources::default()
        });
    };
    let picture = picture(&camera);
    let cutout = person_mask(&camera).and_then(|mask| cut_out(&picture, &mask));
    Ok(Sources {
        screen,
        camera: Some(picture),
        cutout,
    })
}

/// Makes a rendered variant the main cut's thumbnail, `.shot/export/main.png`.
pub fn write_upload(video_dir: &Path, still: &Picture) -> Result<PathBuf, ThumbnailError> {
    let dir = video_dir.join(".shot/cache/thumbnail-studio");
    let io = |path: &Path| {
        let path = path.to_owned();
        move |source| ThumbnailError::Io { path, source }
    };
    fs::create_dir_all(&dir).map_err(io(&dir))?;
    let path = dir.join("upload.png");
    image::RgbaImage::from_raw(still.width, still.height, still.rgba.to_vec())
        .expect("a picture holds width * height pixels")
        .save(&path)
        .map_err(|e| ThumbnailError::Io {
            path: path.clone(),
            source: std::io::Error::other(e),
        })?;
    crate::ship::pick_thumbnail(video_dir, &path).map_err(io(&path))
}

/// The frame of `path` at `at`, `None` when the take has no such track.
fn decoded(
    path: &Path,
    at: Micros,
) -> Result<Option<objc2_core_foundation::CFRetained<CVPixelBuffer>>, ThumbnailError> {
    if !path.exists() {
        return Ok(None);
    }
    let error = |e: shot_media::videotoolbox::DecodeError| ThumbnailError::Decode {
        path: path.to_owned(),
        message: e.to_string(),
    };
    let mut decoder = Decoder::open(path).map_err(error)?;
    Ok(decoder
        .seek(Duration::from_micros(at.0), || false)
        .map_err(error)?
        .map(|frame| frame.buffer))
}

/// A decoded frame as an opaque picture at most [`STUDIO_WIDTH`] wide.
fn picture(buffer: &CVPixelBuffer) -> Picture {
    let planes = Planes::lock(buffer);
    let (width, height) = (planes.width as u32, planes.height as u32);
    let rgb = planes.rgb();
    let rgba: Vec<u8> = rgb
        .as_chunks::<3>()
        .0
        .iter()
        .flat_map(|&[r, g, b]| [r, g, b, 255])
        .collect();
    drop(planes);
    let image = image::RgbaImage::from_raw(width, height, rgba).expect("width * height pixels");
    let image = if width > STUDIO_WIDTH {
        let scaled = height * STUDIO_WIDTH / width;
        image::imageops::resize(
            &image,
            STUDIO_WIDTH,
            scaled,
            image::imageops::FilterType::Triangle,
        )
    } else {
        image
    };
    Picture {
        width: image.width(),
        height: image.height(),
        rgba: std::sync::Arc::new(image.into_raw()),
    }
}

/// Vision's person segmentation of a camera frame at its finest: 0 for background to 255 for
/// the person.
fn person_mask(buffer: &CVPixelBuffer) -> Option<image::GrayImage> {
    let segmenter = PersonSegmenter::new(VNGeneratePersonSegmentationRequestQualityLevel::Accurate);
    let (pixels, (width, height), _) = segmenter.mask(buffer).ok()??;
    image::GrayImage::from_raw(width, height, pixels)
}

/// The person of `frame` under `mask`, cropped to them with room for the outline, or `None`
/// when the mask holds hardly any person.
pub fn cut_out(frame: &Picture, mask: &image::GrayImage) -> Option<Cutout> {
    let (width, height) = (frame.width, frame.height);
    let mask = image::imageops::resize(mask, width, height, image::imageops::FilterType::Triangle);
    let (mut x0, mut y0, mut x1, mut y1) = (width, height, 0, 0);
    let mut covered = 0u64;
    for (x, y, alpha) in mask.enumerate_pixels() {
        if alpha.0[0] > 127 {
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
            covered += 1;
        }
    }
    if covered * 50 < u64::from(width) * u64::from(height) {
        return None;
    }
    // Room for the outline, except where the person runs off the frame's edge.
    let pad = OUTLINE + 2;
    let (x0, y0) = (x0.saturating_sub(pad), y0.saturating_sub(pad));
    let (x1, y1) = ((x1 + pad).min(width), (y1 + pad).min(height));
    let (w, h) = (x1 - x0, y1 - y0);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for y in y0..y1 {
        for x in x0..x1 {
            let at = ((y * width + x) * 4) as usize;
            let alpha = mask.get_pixel(x, y).0[0];
            rgba.extend_from_slice(&frame.rgba[at..at + 3]);
            rgba.push(alpha);
        }
    }
    let person = Picture {
        rgba: std::sync::Arc::new(rgba),
        width: w,
        height: h,
    };
    Some(Cutout {
        outline: silhouette(&person, OUTLINE),
        person,
        crop: Rect::new(
            x0 as f32 / width as f32,
            y0 as f32 / height as f32,
            x1 as f32 / width as f32,
            y1 as f32 / height as f32,
        ),
    })
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
    fn a_cutout_is_the_person_with_room_for_the_outline_but_not_past_the_frame() {
        let frame = Picture {
            rgba: std::sync::Arc::new([10, 20, 30, 255].repeat(100 * 50)),
            width: 100,
            height: 50,
        };
        // The person stands from x 40 to 60 and from y 20 down to the bottom edge.
        let mask = image::GrayImage::from_fn(100, 50, |x, y| {
            image::Luma([if (40..60).contains(&x) && y >= 20 {
                255
            } else {
                0
            }])
        });
        let cutout = cut_out(&frame, &mask).unwrap();
        let pad = OUTLINE + 2;
        assert_eq!(cutout.person.width, 20 + 2 * pad);
        assert_eq!(
            cutout.person.height,
            30 + pad,
            "no room below the frame's edge"
        );
        assert_eq!(cutout.person.rgba[3], 0, "the corner is background");
        assert_eq!(cutout.crop.bottom, 1.0);
        let nobody = image::GrayImage::from_pixel(100, 50, image::Luma([0]));
        assert!(cut_out(&frame, &nobody).is_none());
    }

    #[test]
    fn a_moment_of_the_cut_maps_to_its_take_and_back() {
        let range = |from, to| TimeRange::new(Micros(from), Micros(to)).unwrap();
        let (t1, t2) = (Id::new("t01").unwrap(), Id::new("t02").unwrap());
        let segments = vec![
            (t1.clone(), range(1_000, 5_000)),
            (t2.clone(), range(0, 3_000)),
        ];
        assert_eq!(
            moment(&segments, Micros(500)),
            Some((t1.clone(), Micros(1_500)))
        );
        assert_eq!(
            moment(&segments, Micros(4_500)),
            Some((t2.clone(), Micros(500)))
        );
        assert_eq!(
            moment(&segments, Micros(99_000)),
            Some((t2.clone(), Micros(2_999)))
        );
        assert_eq!(cut_time(&segments, &t2, Micros(500)), Some(Micros(4_500)));
        assert_eq!(cut_time(&segments, &t1, Micros(500)), None, "cut away");
        assert_eq!(moment(&[], Micros(0)), None);
    }

    #[test]
    fn a_candidate_still_names_its_take_and_moment() {
        assert_eq!(
            candidate_moment(Path::new("/x/2-t03-184200.png")),
            Some((Id::new("t03").unwrap(), Micros(184_200_000)))
        );
        assert_eq!(candidate_moment(Path::new("/x/upload.png")), None);
    }

    #[test]
    fn expression_rewards_an_open_mouth_on_a_clean_capture() {
        let closed = openness([(0.0, 0.0), (1.0, 0.0), (0.5, 0.1)].into_iter());
        let open = openness([(0.0, 0.0), (1.0, 0.0), (0.5, 0.6)].into_iter());
        assert!(score(0.8, open) > score(0.8, closed));
        assert_eq!(score(1.0, 1.0), 1.0);
    }
}
