//! One fragmented MP4 per source, written by an `AVAssetWriter`.

use std::fs::File;
use std::path::PathBuf;
use std::ptr::{self, NonNull};
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use block2::RcBlock;
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2_av_foundation::{
    AVAssetWriter, AVAssetWriterInput, AVAssetWriterStatus, AVFileTypeMPEG4,
    AVFileTypeProfileMPEG4AppleHLS, AVMediaTypeAudio, AVMediaTypeVideo,
    AVVideoAllowFrameReorderingKey, AVVideoAverageBitRateKey, AVVideoCodecKey,
    AVVideoCodecTypeHEVC, AVVideoColorPrimaries_ITU_R_709_2, AVVideoColorPrimariesKey,
    AVVideoColorPropertiesKey, AVVideoCompressionPropertiesKey, AVVideoExpectedSourceFrameRateKey,
    AVVideoHeightKey, AVVideoMaxKeyFrameIntervalDurationKey, AVVideoTransferFunction_ITU_R_709_2,
    AVVideoTransferFunctionKey, AVVideoWidthKey, AVVideoYCbCrMatrix_ITU_R_709_2,
    AVVideoYCbCrMatrixKey,
};
use objc2_avf_audio::{AVEncoderBitRateKey, AVFormatIDKey, AVNumberOfChannelsKey, AVSampleRateKey};
use objc2_core_audio_types::kAudioFormatMPEG4AAC;
use objc2_core_foundation::CFRetained;
use objc2_core_media::{
    CMSampleBuffer, CMSampleTimingInfo, CMTime, CMTimeFlags, CMVideoFormatDescription,
    CMVideoFormatDescriptionCreateForImageBuffer, CMVideoFormatDescriptionGetDimensions,
    kCMTimeInvalid, kCMTimeZero,
};
use objc2_core_video::CVImageBuffer;
use objc2_foundation::{NSDictionary, NSNumber, NSString};
use objc2_uniform_type_identifiers::UTType;

use super::sink::SegmentSink;

use crate::clock::{MediaClock, nanos};
use crate::{CaptureError, Track as TrackSummary};

/// Fragment length and keyframe interval, in seconds.
const FRAGMENT: f64 = 0.5;
pub const AUDIO_RATE: f64 = 48_000.0;

#[derive(Debug, Clone, Copy)]
pub enum Spec {
    /// HEVC at the first frame's size. Bitrate follows from bits per pixel per frame.
    Video {
        fps: u32,
        bits_per_pixel: f64,
    },
    Audio {
        channels: u32,
        bitrate: u32,
    },
}

/// One source's file. Samples before the origin are dropped; the writer opens on the first
/// sample after it, so video takes its size from the frames.
pub struct Track {
    pub file: &'static str,
    path: PathBuf,
    spec: Spec,
    origin: OnceLock<CMTime>,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    writer: Option<Writer>,
    error: Option<String>,
    samples: u64,
    dropped: u64,
}

struct Writer {
    writer: Retained<AVAssetWriter>,
    input: Retained<AVAssetWriterInput>,
    /// The writer holds its delegate weakly.
    segments: Retained<SegmentSink>,
}

// SAFETY: the writer is only used behind the track's mutex, and AVAssetWriter may be driven from
// any thread as long as calls are serialized.
unsafe impl Send for Writer {}

impl Track {
    pub fn new(dir: &std::path::Path, file: &'static str, spec: Spec) -> Self {
        Self {
            file,
            path: dir.join(file),
            spec,
            origin: OnceLock::new(),
            state: Mutex::default(),
        }
    }

    /// Sets media time zero, expressed in this source's clock. Samples before it are dropped.
    pub fn set_origin(&self, origin: CMTime) {
        let _ = self.origin.set(origin);
    }

    /// The clock that turns this source's timestamps into media time, once the origin is set.
    pub fn clock(&self) -> Option<MediaClock> {
        self.origin
            .get()
            .map(|o| MediaClock::new(nanos(o.value, o.timescale)))
    }

    /// Appends a sample stamped on the source's clock, moved to media time.
    pub fn push(&self, sample: &CMSampleBuffer) {
        let (Some(&origin), Some(clock)) = (self.origin.get(), self.clock()) else {
            return;
        };
        let pts = unsafe { sample.presentation_time_stamp() };
        if clock.media_time(nanos(pts.value, pts.timescale)).is_none() {
            return;
        }
        if let Some(sample) = retimed(sample, origin) {
            self.append(&sample);
        }
    }

    /// Appends a video frame shown from media time `at` for `duration`.
    pub fn push_frame(&self, image: &CVImageBuffer, at: CMTime, duration: CMTime) {
        if let Some(sample) = frame_sample(image, at, duration) {
            self.append(&sample);
        }
    }

    /// Opens the writer for frames like `image` ahead of the first append, which would otherwise
    /// stall for the encoder setup.
    pub fn prepare(&self, image: &CVImageBuffer) {
        let mut state = self.state.lock().expect("track poisoned");
        if state.writer.is_some() || state.error.is_some() {
            return;
        }
        let Some(sample) = frame_sample(image, unsafe { kCMTimeZero }, unsafe { kCMTimeZero })
        else {
            return;
        };
        match Writer::open(&self.path, self.spec, &sample) {
            Ok(writer) => state.writer = Some(writer),
            Err(message) => state.error = Some(message),
        }
    }

    /// Appends a sample in media time, opening the writer on the first one.
    fn append(&self, sample: &CMSampleBuffer) {
        let mut state = self.state.lock().expect("track poisoned");
        if state.error.is_some() {
            return;
        }
        if state.writer.is_none() {
            match Writer::open(&self.path, self.spec, sample) {
                Ok(writer) => state.writer = Some(writer),
                Err(message) => {
                    state.error = Some(message);
                    return;
                }
            }
        }
        let writer = state.writer.as_ref().expect("opened above");
        if !unsafe { writer.input.isReadyForMoreMediaData() } {
            state.dropped += 1;
        } else if unsafe { writer.input.appendSampleBuffer(sample) } {
            state.samples += 1;
        } else {
            state.error = Some(writer.error_message());
        }
    }

    /// Finishes the file. A track that never received a sample writes no file.
    pub fn finish(&self) -> Result<TrackSummary, CaptureError> {
        let mut state = self.state.lock().expect("track poisoned");
        let fail = |message| CaptureError::Write {
            file: self.file,
            message,
        };
        if let Some(message) = state.error.take() {
            return Err(fail(message));
        }
        if let Some(writer) = state.writer.take() {
            writer.finish().map_err(fail)?;
        }
        Ok(TrackSummary {
            file: self.file,
            samples: state.samples,
            dropped: state.dropped,
        })
    }
}

impl Writer {
    fn open(path: &std::path::Path, spec: Spec, first: &CMSampleBuffer) -> Result<Self, String> {
        let file = File::create(path).map_err(|e| e.to_string())?;
        let segments = SegmentSink::new(file);
        let mpeg4 = UTType::typeWithIdentifier(
            unsafe { AVFileTypeMPEG4 }.expect("AVFoundation defines MPEG-4"),
        )
        .ok_or("MPEG-4 has no UTType")?;
        let writer = unsafe { AVAssetWriter::initWithContentType(AVAssetWriter::alloc(), &mpeg4) };
        let format = unsafe { first.format_description() };
        let (media_type, settings) = match spec {
            Spec::Video {
                fps,
                bits_per_pixel,
            } => {
                let format = format.as_deref().ok_or("the first frame has no format")?;
                let size = unsafe { CMVideoFormatDescriptionGetDimensions(format) };
                let bitrate = f64::from(size.width) * f64::from(size.height) * f64::from(fps);
                (
                    unsafe { AVMediaTypeVideo }.expect("AVFoundation defines video"),
                    video_settings(size.width, size.height, fps, bitrate * bits_per_pixel),
                )
            }
            Spec::Audio { channels, bitrate } => (
                unsafe { AVMediaTypeAudio }.expect("AVFoundation defines audio"),
                audio_settings(channels, bitrate),
            ),
        };
        let input = unsafe {
            AVAssetWriterInput::initWithMediaType_outputSettings_sourceFormatHint(
                AVAssetWriterInput::alloc(),
                media_type,
                Some(&settings),
                format.as_deref(),
            )
        };
        unsafe {
            input.setExpectsMediaDataInRealTime(true);
            // Segments of moof and mdat go to the delegate, which appends them to the file.
            let fragment = CMTime::with_seconds(FRAGMENT, 600);
            writer.setOutputFileTypeProfile(Some(
                AVFileTypeProfileMPEG4AppleHLS.expect("AVFoundation defines the HLS profile"),
            ));
            writer.setPreferredOutputSegmentInterval(fragment);
            writer.setInitialSegmentStartTime(kCMTimeZero);
            writer.setDelegate(Some(ProtocolObject::from_ref(&*segments)));
            if !writer.canAddInput(&input) {
                return Err("the writer refused the track settings".into());
            }
            writer.addInput(&input);
        }
        let this = Self {
            writer,
            input,
            segments,
        };
        if !unsafe { this.writer.startWriting() } {
            return Err(this.error_message());
        }
        unsafe { this.writer.startSessionAtSourceTime(kCMTimeZero) };
        Ok(this)
    }

    fn error_message(&self) -> String {
        unsafe { self.writer.error() }
            .map(|e| e.localizedDescription().to_string())
            .or_else(|| self.segments.error())
            .unwrap_or_else(|| "unknown writer error".into())
    }

    fn finish(self) -> Result<(), String> {
        unsafe { self.input.markAsFinished() };
        let (done, wait) = mpsc::channel();
        let handler = RcBlock::new(move || {
            let _ = done.send(());
        });
        unsafe { self.writer.finishWritingWithCompletionHandler(&handler) };
        wait.recv_timeout(Duration::from_secs(10))
            .map_err(|_| "finishing the file timed out".to_string())?;
        match unsafe { self.writer.status() } {
            AVAssetWriterStatus::Completed => self.segments.error().map_or(Ok(()), Err),
            _ => Err(self.error_message()),
        }
    }
}

fn key(key: Option<&'static NSString>) -> &'static NSString {
    key.expect("AVFoundation defines its settings keys")
}

fn dict(pairs: &[(&NSString, &AnyObject)]) -> Retained<NSDictionary<NSString, AnyObject>> {
    let (keys, values): (Vec<_>, Vec<_>) = pairs.iter().copied().unzip();
    NSDictionary::from_slices(&keys, &values)
}

fn video_settings(
    width: i32,
    height: i32,
    fps: u32,
    bitrate: f64,
) -> Retained<NSDictionary<NSString, AnyObject>> {
    unsafe {
        let compression = dict(&[
            (key(AVVideoAverageBitRateKey), &NSNumber::new_f64(bitrate)),
            (
                key(AVVideoMaxKeyFrameIntervalDurationKey),
                &NSNumber::new_f64(FRAGMENT),
            ),
            (
                key(AVVideoAllowFrameReorderingKey),
                &NSNumber::new_bool(false),
            ),
            (
                key(AVVideoExpectedSourceFrameRateKey),
                &NSNumber::new_u32(fps),
            ),
        ]);
        let color = dict(&[
            (
                key(AVVideoColorPrimariesKey),
                key(AVVideoColorPrimaries_ITU_R_709_2),
            ),
            (
                key(AVVideoTransferFunctionKey),
                key(AVVideoTransferFunction_ITU_R_709_2),
            ),
            (
                key(AVVideoYCbCrMatrixKey),
                key(AVVideoYCbCrMatrix_ITU_R_709_2),
            ),
        ]);
        dict(&[
            (
                key(AVVideoCodecKey),
                AVVideoCodecTypeHEVC.expect("AVFoundation defines HEVC"),
            ),
            (key(AVVideoWidthKey), &NSNumber::new_i32(width)),
            (key(AVVideoHeightKey), &NSNumber::new_i32(height)),
            (key(AVVideoCompressionPropertiesKey), &compression),
            (key(AVVideoColorPropertiesKey), &color),
        ])
    }
}

fn audio_settings(channels: u32, bitrate: u32) -> Retained<NSDictionary<NSString, AnyObject>> {
    unsafe {
        dict(&[
            (key(AVFormatIDKey), &NSNumber::new_u32(kAudioFormatMPEG4AAC)),
            (key(AVSampleRateKey), &NSNumber::new_f64(AUDIO_RATE)),
            (key(AVNumberOfChannelsKey), &NSNumber::new_u32(channels)),
            (key(AVEncoderBitRateKey), &NSNumber::new_u32(bitrate)),
        ])
    }
}

/// A copy of `sample` with every timestamp moved back by `origin`.
fn retimed(sample: &CMSampleBuffer, origin: CMTime) -> Option<CFRetained<CMSampleBuffer>> {
    let mut count = 0;
    unsafe {
        sample.sample_timing_info_array(0, ptr::null_mut(), &mut count);
        let invalid = CMSampleTimingInfo {
            duration: kCMTimeInvalid,
            presentationTimeStamp: kCMTimeInvalid,
            decodeTimeStamp: kCMTimeInvalid,
        };
        let mut timing = vec![invalid; usize::try_from(count).ok()?];
        if sample.sample_timing_info_array(count, timing.as_mut_ptr(), ptr::null_mut()) != 0 {
            return None;
        }
        for t in &mut timing {
            t.presentationTimeStamp = t.presentationTimeStamp.subtract(origin);
            if t.decodeTimeStamp.flags.contains(CMTimeFlags::Valid) {
                t.decodeTimeStamp = t.decodeTimeStamp.subtract(origin);
            }
        }
        let mut out: *mut CMSampleBuffer = ptr::null_mut();
        if CMSampleBuffer::create_copy_with_new_timing(
            None,
            sample,
            count,
            timing.as_ptr(),
            NonNull::from(&mut out),
        ) != 0
        {
            return None;
        }
        NonNull::new(out).map(|out| CFRetained::from_raw(out))
    }
}

/// A sample buffer that shows `image` from `at` for `duration`.
fn frame_sample(
    image: &CVImageBuffer,
    at: CMTime,
    duration: CMTime,
) -> Option<CFRetained<CMSampleBuffer>> {
    let mut format: *const CMVideoFormatDescription = ptr::null();
    let mut timing = CMSampleTimingInfo {
        duration,
        presentationTimeStamp: at,
        decodeTimeStamp: unsafe { kCMTimeInvalid },
    };
    let mut sample: *mut CMSampleBuffer = ptr::null_mut();
    unsafe {
        if CMVideoFormatDescriptionCreateForImageBuffer(None, image, NonNull::from(&mut format))
            != 0
        {
            return None;
        }
        let format = CFRetained::from_raw(NonNull::new(format.cast_mut())?);
        if CMSampleBuffer::create_ready_with_image_buffer(
            None,
            image,
            &format,
            NonNull::from(&mut timing),
            NonNull::from(&mut sample),
        ) != 0
        {
            return None;
        }
        NonNull::new(sample).map(|sample| CFRetained::from_raw(sample))
    }
}
