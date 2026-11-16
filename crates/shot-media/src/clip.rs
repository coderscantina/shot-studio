//! Reading footage from outside, a phone clip first of all, as frames a take can hold.
//!
//! An `AVAssetReaderVideoCompositionOutput` renders the clip through a video composition built
//! from the asset: the track's transform turns the picture upright, the composition's BT.709
//! color properties make AVFoundation tone-map HDR down to SDR, and its frame duration caps the
//! rate. The composition still follows the clip's own timestamps, so a variable-rate clip comes
//! out with gaps; the reader fills a constant-rate grid from it, each slot showing the frame on
//! screen at its time. Frames are IOSurface-backed video range 4:2:0, the input format of
//! [`crate::encode::HevcEncoder`].

use std::path::Path;
use std::time::Duration;

use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::AnyObject;
use objc2_av_foundation::{
    AVAssetReader, AVAssetReaderStatus, AVAssetReaderVideoCompositionOutput, AVAssetTrack,
    AVMediaTypeAudio, AVMediaTypeVideo, AVMutableVideoComposition, AVURLAsset,
    AVVideoColorPrimaries_ITU_R_709_2, AVVideoTransferFunction_ITU_R_709_2,
    AVVideoYCbCrMatrix_ITU_R_709_2,
};
use objc2_core_foundation::{CFRetained, CFString, CGSize};
use objc2_core_media::{CMTime, kCMPersistentTrackID_Invalid};
use objc2_core_video::{
    CVPixelBuffer, kCVPixelBufferIOSurfacePropertiesKey, kCVPixelBufferMetalCompatibilityKey,
    kCVPixelBufferPixelFormatTypeKey,
};
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString, NSURL};

use crate::encode::INPUT_PIXEL_FORMAT;
use crate::videotoolbox::Frame;

#[derive(Debug, thiserror::Error)]
pub enum ClipError {
    #[error("{0} has no video track AVFoundation can read")]
    NoVideoTrack(String),
    #[error("reading {0} failed: {1}")]
    Reader(String, String),
}

/// The frame rate a clip of `nominal` fps becomes: 30 up to 30 fps, 60 above, so frames sit on
/// the 60 fps grid the editor counts.
pub fn frame_rate(nominal: f32) -> u32 {
    if nominal > 30.5 { 60 } else { 30 }
}

/// Upright, constant-rate, SDR BT.709 frames of a clip's first video track.
pub struct ClipReader {
    path: String,
    reader: Retained<AVAssetReader>,
    output: Retained<AVAssetReaderVideoCompositionOutput>,
    width: u32,
    height: u32,
    fps: u32,
    /// Frames on the constant-rate grid, to the end of the video track.
    frames: u64,
    has_audio: bool,
    /// The next grid slot to hand out.
    next: u64,
    /// The composition's frame on screen at the last slot, and the one after it.
    shown: Option<CFRetained<CVPixelBuffer>>,
    upcoming: Option<Frame>,
}

impl ClipReader {
    pub fn open(path: &Path) -> Result<Self, ClipError> {
        // AVFoundation autoreleases objects, and a plain thread has no pool to drain them.
        autoreleasepool(|_| Self::open_in_pool(path))
    }

    fn open_in_pool(path: &Path) -> Result<Self, ClipError> {
        let name = path.display().to_string();
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        let asset = unsafe { AVURLAsset::URLAssetWithURL_options(&url, None) };
        let video = unsafe { AVMediaTypeVideo }.expect("AVFoundation defines the video type");
        let audio = unsafe { AVMediaTypeAudio }.expect("AVFoundation defines the audio type");
        // See the decoder: a local file loads at once, and the async loader's block is not Send.
        #[allow(deprecated)]
        let (track, has_audio) = unsafe {
            (
                asset.tracksWithMediaType(video).firstObject(),
                asset.tracksWithMediaType(audio).count() > 0,
            )
        };
        let track = track.ok_or_else(|| ClipError::NoVideoTrack(name.clone()))?;
        let (width, height) = upright_size(&track);
        let fps = frame_rate(unsafe { track.nominalFrameRate() });

        #[allow(deprecated)]
        let composition =
            unsafe { AVMutableVideoComposition::videoCompositionWithPropertiesOfAsset(&asset) };
        unsafe {
            composition.setSourceTrackIDForFrameTiming(kCMPersistentTrackID_Invalid);
            composition.setFrameDuration(CMTime::new(1, fps as i32));
            composition.setRenderSize(CGSize {
                width: f64::from(width),
                height: f64::from(height),
            });
            composition.setColorPrimaries(AVVideoColorPrimaries_ITU_R_709_2);
            composition.setColorTransferFunction(AVVideoTransferFunction_ITU_R_709_2);
            composition.setColorYCbCrMatrix(AVVideoYCbCrMatrix_ITU_R_709_2);
        }

        let reader = unsafe { AVAssetReader::assetReaderWithAsset_error(&asset) }
            .map_err(|error| ClipError::Reader(name.clone(), describe(&error)))?;
        let output = unsafe {
            AVAssetReaderVideoCompositionOutput::assetReaderVideoCompositionOutputWithVideoTracks_videoSettings(
                &NSArray::from_slice(&[&*track]),
                Some(&pixel_settings()),
            )
        };
        unsafe {
            output.setAlwaysCopiesSampleData(false);
            output.setVideoComposition(Some(&composition));
            reader.addOutput(&output);
            if !reader.startReading() {
                return Err(reader_error(&name, &reader));
            }
        }
        let range = unsafe { track.timeRange() };
        let end = to_duration(unsafe { range.start.add(range.duration) });
        let mut clip = Self {
            path: name,
            reader,
            output,
            width,
            height,
            fps,
            frames: slots(end, fps),
            has_audio,
            next: 0,
            shown: None,
            upcoming: None,
        };
        clip.upcoming = clip.read()?;
        Ok(clip)
    }

    /// The upright picture size, even in both directions as 4:2:0 needs.
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// The constant rate frames come at, see [`frame_rate`].
    pub fn fps(&self) -> u32 {
        self.fps
    }

    /// The length of the constant-rate frames.
    pub fn duration(&self) -> Duration {
        Duration::from_secs(self.frames) / self.fps
    }

    pub fn has_audio(&self) -> bool {
        self.has_audio
    }

    /// The frame of the next grid slot, or `None` after the last one. A slot shows the latest
    /// frame due by half a frame after its time, the first frame before that.
    pub fn next_frame(&mut self) -> Result<Option<Frame>, ClipError> {
        if self.next == self.frames {
            return Ok(None);
        }
        let pts = Duration::from_secs(self.next) / self.fps;
        let due = pts + Duration::from_secs(1) / (2 * self.fps);
        while let Some(upcoming) = self
            .upcoming
            .take_if(|f| f.pts <= due || self.shown.is_none())
        {
            self.shown = Some(upcoming.buffer);
            self.upcoming = self.read()?;
        }
        self.next += 1;
        Ok(self.shown.clone().map(|buffer| Frame { pts, buffer }))
    }

    /// The composition's next frame.
    fn read(&mut self) -> Result<Option<Frame>, ClipError> {
        autoreleasepool(|_| {
            loop {
                let Some(sample) = (unsafe { self.output.copyNextSampleBuffer() }) else {
                    if unsafe { self.reader.status() } == AVAssetReaderStatus::Failed {
                        return Err(reader_error(&self.path, &self.reader));
                    }
                    return Ok(None);
                };
                if let Some(buffer) = unsafe { sample.image_buffer() } {
                    let pts = to_duration(unsafe { sample.presentation_time_stamp() });
                    return Ok(Some(Frame { pts, buffer }));
                }
            }
        })
    }
}

impl Drop for ClipReader {
    fn drop(&mut self) {
        unsafe { self.reader.cancelReading() };
    }
}

/// Slots of `1/fps` from zero to `end`, the last one counted when it is at least half there.
fn slots(end: Duration, fps: u32) -> u64 {
    (end.as_secs_f64() * f64::from(fps)).round() as u64
}

/// The track's natural size turned by its preferred transform, rounded down to even.
fn upright_size(track: &AVAssetTrack) -> (u32, u32) {
    let (size, t) = unsafe { (track.naturalSize(), track.preferredTransform()) };
    let width = (t.a * size.width + t.c * size.height).abs().round() as u32;
    let height = (t.b * size.width + t.d * size.height).abs().round() as u32;
    (width & !1, height & !1)
}

/// IOSurface-backed, Metal compatible frames in the encoder's input format.
fn pixel_settings() -> Retained<NSDictionary<NSString, AnyObject>> {
    let format = NSNumber::new_u32(INPUT_PIXEL_FORMAT);
    let yes = NSNumber::new_bool(true);
    let no_surface_options = NSDictionary::<NSString, AnyObject>::new();
    // SAFETY: Constant CF strings from CoreVideo.
    let keys = unsafe {
        [
            kCVPixelBufferPixelFormatTypeKey,
            kCVPixelBufferIOSurfacePropertiesKey,
            kCVPixelBufferMetalCompatibilityKey,
        ]
    };
    let values: [&AnyObject; 3] = [&format, &no_surface_options, &yes];
    NSDictionary::from_slices(&keys.map(ns), &values)
}

fn ns(key: &CFString) -> &NSString {
    // SAFETY: CFString and NSString are toll-free bridged.
    unsafe { &*(key as *const CFString).cast::<NSString>() }
}

fn reader_error(path: &str, reader: &AVAssetReader) -> ClipError {
    let message = unsafe { reader.error() }
        .map(|error| describe(&error))
        .unwrap_or_else(|| "unknown reader error".into());
    ClipError::Reader(path.to_owned(), message)
}

fn describe(error: &objc2_foundation::NSError) -> String {
    error.localizedDescription().to_string()
}

fn to_duration(time: CMTime) -> Duration {
    if time.value <= 0 || time.timescale <= 0 {
        return Duration::ZERO;
    }
    Duration::from_nanos((time.value as u128 * 1_000_000_000 / time.timescale as u128) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clips_up_to_30_fps_become_30_and_faster_ones_60() {
        assert_eq!(frame_rate(23.976), 30);
        assert_eq!(frame_rate(29.97), 30);
        assert_eq!(frame_rate(30.0), 30);
        assert_eq!(frame_rate(59.94), 60);
        assert_eq!(frame_rate(120.0), 60);
    }

    #[test]
    fn a_clip_fills_the_grid_to_the_end_of_its_video() {
        assert_eq!(slots(Duration::from_secs_f64(19.986633), 30), 600);
        assert_eq!(slots(Duration::from_secs(2), 60), 120);
        assert_eq!(slots(Duration::from_millis(10), 30), 0);
    }
}
