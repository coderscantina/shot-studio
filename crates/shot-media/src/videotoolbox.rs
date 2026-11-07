//! Hardware decode: `AVAssetReader` hands over compressed samples untouched, a
//! `VTDecompressionSession` that must run on the hardware decoder turns them into
//! IOSurface-backed `CVPixelBuffer`s that Metal samples in place. No pixel ever touches the CPU.
//!
//! The decoder outputs video range, so VideoToolbox runs a hardware pixel transfer to full range.
//! [`Matrix::Bt601`] lets the same transfer also re-encode the colors for gpui's surface shader.

use std::collections::VecDeque;
use std::path::Path;
use std::ptr::{self, NonNull};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::{Retained, autoreleasepool};
use objc2_av_foundation::{
    AVAssetReader, AVAssetReaderStatus, AVAssetReaderTrackOutput, AVAssetTrack, AVMediaTypeVideo,
    AVURLAsset,
};
use objc2_core_foundation::{
    CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType, kCFBooleanTrue,
};
use objc2_core_media::{
    CMFormatDescription, CMSampleBuffer, CMTime, CMTimeRange, kCMSampleAttachmentKey_NotSync,
    kCMTimePositiveInfinity,
};
use objc2_core_video::{
    CVImageBuffer, CVPixelBuffer, CVPixelBufferGetHeight, CVPixelBufferGetIOSurface,
    CVPixelBufferGetPixelFormatType, CVPixelBufferGetWidth, kCVImageBufferYCbCrMatrix_ITU_R_601_4,
    kCVImageBufferYCbCrMatrixKey, kCVPixelBufferIOSurfacePropertiesKey,
    kCVPixelBufferMetalCompatibilityKey, kCVPixelBufferPixelFormatTypeKey,
    kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
};
use objc2_foundation::{NSString, NSURL};
use objc2_video_toolbox::{
    VTDecodeFrameFlags, VTDecodeInfoFlags, VTDecompressionSession, VTSessionCopyProperty,
    VTSessionSetProperty, kVTDecompressionPropertyKey_PixelTransferProperties,
    kVTDecompressionPropertyKey_UsingHardwareAcceleratedVideoDecoder,
    kVTPixelTransferPropertyKey_DestinationYCbCrMatrix,
    kVTVideoDecoderSpecification_RequireHardwareAcceleratedVideoDecoder,
};

use crate::timing::FrameIndex;

/// The decoder's output format. gpui's surface shader accepts only full-range biplanar 4:2:0.
pub const PIXEL_FORMAT: u32 = kCVPixelFormatType_420YpCbCr8BiPlanarFullRange;

/// The YCbCr matrix of decoded frames.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Matrix {
    /// The source's own matrix, BT.709 for our recordings.
    #[default]
    Source,
    /// BT.601, the matrix gpui's surface shader decodes with. The hardware pixel transfer
    /// re-encodes each frame into it, so a BT.709 source shows its true colors in gpui.
    Bt601,
}

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("{0} has no video track")]
    NoVideoTrack(String),
    #[error("reading samples failed: {0}")]
    Reader(String),
    #[error("creating the hardware decoder failed with OSStatus {0}")]
    Session(i32),
    #[error("configuring the decoder's pixel transfer failed with OSStatus {0}")]
    PixelTransfer(i32),
    #[error("decoding a frame failed with OSStatus {0}")]
    Decode(i32),
}

/// A decoded frame in an IOSurface owned by CoreVideo.
pub struct Frame {
    /// Presentation time from the start of the stream.
    pub pts: Duration,
    pub buffer: CFRetained<CVPixelBuffer>,
}

// SAFETY: CoreVideo buffers are reference counted atomically and nothing mutates a decoded frame,
// so moving the handle to the UI thread is sound.
unsafe impl Send for Frame {}

impl Frame {
    pub fn width(&self) -> usize {
        CVPixelBufferGetWidth(&self.buffer)
    }

    pub fn height(&self) -> usize {
        CVPixelBufferGetHeight(&self.buffer)
    }

    pub fn pixel_format(&self) -> u32 {
        CVPixelBufferGetPixelFormatType(&self.buffer)
    }

    pub fn is_iosurface_backed(&self) -> bool {
        CVPixelBufferGetIOSurface(Some(&self.buffer)).is_some()
    }

    /// The YCbCr matrix the buffer is tagged with, such as `ITU_R_709_2`.
    pub fn ycbcr_matrix(&self) -> Option<String> {
        // SAFETY: A null mode pointer is allowed, and the key names a CFString attachment.
        let value = unsafe {
            self.buffer
                .attachment(kCVImageBufferYCbCrMatrixKey, ptr::null_mut())
        }?;
        value
            .downcast::<CFString>()
            .ok()
            .map(|name| name.to_string())
    }
}

/// Frames in flight on the hardware decoder during sequential decode.
const PIPELINE_DEPTH: usize = 4;

/// Decoder output in decode order. `Ok(None)` marks a frame the decoder dropped.
#[derive(Default)]
struct Outputs {
    queue: Mutex<VecDeque<Result<Option<Frame>, i32>>>,
    ready: Condvar,
}

type OutputHandler = RcBlock<dyn Fn(i32, VTDecodeInfoFlags, *mut CVImageBuffer, CMTime, CMTime)>;

/// Sequential and seeking decode of one video track.
pub struct Decoder {
    asset: Retained<AVURLAsset>,
    track: Retained<AVAssetTrack>,
    index: FrameIndex,
    frame_duration: Duration,
    session: CFRetained<VTDecompressionSession>,
    reader: Retained<AVAssetReader>,
    output: Retained<AVAssetReaderTrackOutput>,
    outputs: Arc<Outputs>,
    deliver: OutputHandler,
    discard: OutputHandler,
    /// Submitted frames whose output has not been taken yet.
    in_flight: usize,
    end_of_track: bool,
}

impl Decoder {
    /// Opens the first video track, indexes its keyframes and starts at the first frame.
    pub fn open(path: &Path) -> Result<Self, DecodeError> {
        Self::open_with(path, Matrix::Source)
    }

    /// Like [`Decoder::open`], with frames in the given YCbCr matrix.
    pub fn open_with(path: &Path, matrix: Matrix) -> Result<Self, DecodeError> {
        // AVFoundation autoreleases objects, and a plain thread has no pool to drain them.
        autoreleasepool(|_| Self::open_in_pool(path, matrix))
    }

    fn open_in_pool(path: &Path, matrix: Matrix) -> Result<Self, DecodeError> {
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        let asset = unsafe { AVURLAsset::URLAssetWithURL_options(&url, None) };
        let media_type = unsafe { AVMediaTypeVideo }.expect("AVFoundation defines the video type");
        // The async loader hands the track to a block on another thread, and AVAssetTrack is not
        // Send in objc2. A local file loads at once, so the synchronous call costs nothing.
        #[allow(deprecated)]
        let track = unsafe { asset.tracksWithMediaType(media_type) }
            .firstObject()
            .ok_or_else(|| DecodeError::NoVideoTrack(path.display().to_string()))?;

        // One passthrough pass reads timing and sync flags. It reads compressed bytes only.
        let (reader, output) = start_reader(&asset, &track, Duration::ZERO)?;
        let mut samples = Vec::new();
        let mut format = None;
        while let Some(sample) = next_sample(&reader, &output)? {
            format = format.or_else(|| unsafe { sample.format_description() });
            samples.push((
                to_duration(unsafe { sample.presentation_time_stamp() }),
                is_keyframe(&sample),
            ));
        }
        let index = FrameIndex::from_samples(samples);
        let format = format.ok_or_else(|| DecodeError::NoVideoTrack(path.display().to_string()))?;
        let frame_duration = match index.frames() {
            [first, second, ..] => *second - *first,
            _ => Duration::from_secs(1) / 60,
        };

        let outputs = Arc::new(Outputs::default());
        let deliver = {
            let outputs = outputs.clone();
            RcBlock::new(
                move |status: i32,
                      _info: VTDecodeInfoFlags,
                      image: *mut CVImageBuffer,
                      pts: CMTime,
                      _duration: CMTime| {
                    let result = match status {
                        0 => Ok(NonNull::new(image).map(|image| Frame {
                            pts: to_duration(pts),
                            // SAFETY: VideoToolbox passes a valid buffer, retained before the block returns.
                            buffer: unsafe { CFRetained::retain(image) },
                        })),
                        status => Err(status),
                    };
                    let mut queue = outputs.queue.lock().expect("decoder output poisoned");
                    queue.push_back(result);
                    outputs.ready.notify_one();
                },
            )
        };
        let discard = RcBlock::new(
            |_: i32, _: VTDecodeInfoFlags, _: *mut CVImageBuffer, _: CMTime, _: CMTime| {},
        );
        let (reader, output) = start_reader(&asset, &track, Duration::ZERO)?;
        Ok(Self {
            session: create_session(&format, matrix)?,
            asset,
            track,
            index,
            frame_duration,
            reader,
            output,
            outputs,
            deliver,
            discard,
            in_flight: 0,
            end_of_track: false,
        })
    }

    pub fn index(&self) -> &FrameIndex {
        &self.index
    }

    pub fn frame_duration(&self) -> Duration {
        self.frame_duration
    }

    /// Time from the first frame to the end of the last one.
    pub fn duration(&self) -> Duration {
        self.index.frames().last().copied().unwrap_or_default() + self.frame_duration
    }

    /// Whether VideoToolbox reports this session on the hardware decoder.
    pub fn uses_hardware(&self) -> bool {
        let mut value: *const CFType = ptr::null();
        // SAFETY: The key names a CFBoolean property and the out pointer receives a +1 reference.
        let status = unsafe {
            VTSessionCopyProperty(
                &self.session,
                kVTDecompressionPropertyKey_UsingHardwareAcceleratedVideoDecoder,
                None,
                (&raw mut value).cast(),
            )
        };
        let Some(value) = NonNull::new(value.cast_mut()) else {
            return false;
        };
        // SAFETY: Copy functions return an owned reference.
        let value = unsafe { CFRetained::from_raw(value) };
        status == 0
            && value
                .downcast::<CFBoolean>()
                .is_ok_and(|value| value.as_bool())
    }

    /// Decodes the next frame in presentation order, or `None` at the end of the track.
    /// Keeps a few frames in flight so the hardware decoder works on them in parallel.
    pub fn next_frame(&mut self) -> Result<Option<Frame>, DecodeError> {
        autoreleasepool(|_| self.next_frame_in_pool())
    }

    fn next_frame_in_pool(&mut self) -> Result<Option<Frame>, DecodeError> {
        loop {
            while !self.end_of_track && self.in_flight < PIPELINE_DEPTH {
                match next_sample(&self.reader, &self.output)? {
                    Some(sample) => self.submit(&sample, true)?,
                    None => self.end_of_track = true,
                }
            }
            if self.in_flight == 0 {
                return Ok(None);
            }
            if let Some(frame) = self.take_output()? {
                return Ok(Some(frame));
            }
        }
    }

    /// Returns the frame on screen at `time`, decoding forward from the keyframe before it.
    /// Frames before the target are decoded without output. Gives up with `None` as soon as
    /// `cancelled` returns true, so a newer seek can start.
    pub fn seek(
        &mut self,
        time: Duration,
        cancelled: impl Fn() -> bool,
    ) -> Result<Option<Frame>, DecodeError> {
        // Each seek starts a new reader, whose autoreleased objects would pile up without a pool.
        autoreleasepool(|_| self.seek_in_pool(time, cancelled))
    }

    fn seek_in_pool(
        &mut self,
        time: Duration,
        cancelled: impl Fn() -> bool,
    ) -> Result<Option<Frame>, DecodeError> {
        let Some(plan) = self.index.plan(time) else {
            return Ok(None);
        };
        self.drain();
        (self.reader, self.output) = start_reader(&self.asset, &self.track, plan.keyframe)?;
        while let Some(sample) = next_sample(&self.reader, &self.output)? {
            if to_duration(unsafe { sample.presentation_time_stamp() }) >= plan.target {
                self.submit(&sample, true)?;
                return match self.take_output()? {
                    Some(frame) => Ok(Some(frame)),
                    None => self.next_frame_in_pool(),
                };
            }
            if cancelled() {
                self.drain();
                return Ok(None);
            }
            self.submit(&sample, false)?;
        }
        Ok(None)
    }

    fn submit(&mut self, sample: &CMSampleBuffer, deliver: bool) -> Result<(), DecodeError> {
        let mut flags = VTDecodeFrameFlags::Frame_EnableAsynchronousDecompression;
        let handler = if deliver {
            &self.deliver
        } else {
            flags |= VTDecodeFrameFlags::Frame_DoNotOutputFrame;
            &self.discard
        };
        // SAFETY: The sample comes from this track's reader, and the session retains the handler
        // until it ran. On an error return the handler is never called.
        let status = unsafe {
            self.session.decode_frame_with_output_handler(
                sample,
                flags,
                ptr::null_mut(),
                RcBlock::as_ptr(handler),
            )
        };
        if status != 0 {
            return Err(DecodeError::Decode(status));
        }
        if deliver {
            self.in_flight += 1;
        }
        Ok(())
    }

    /// Waits for the oldest submitted frame. `None` means the decoder dropped it.
    fn take_output(&mut self) -> Result<Option<Frame>, DecodeError> {
        let mut queue = self.outputs.queue.lock().expect("decoder output poisoned");
        let result = loop {
            match queue.pop_front() {
                Some(result) => break result,
                None => {
                    queue = self
                        .outputs
                        .ready
                        .wait(queue)
                        .expect("decoder output poisoned")
                }
            }
        };
        self.in_flight -= 1;
        result.map_err(DecodeError::Decode)
    }

    /// Finishes and drops everything in flight, before reading from a new position.
    fn drain(&mut self) {
        unsafe { self.session.wait_for_asynchronous_frames() };
        self.outputs
            .queue
            .lock()
            .expect("decoder output poisoned")
            .clear();
        self.in_flight = 0;
        self.end_of_track = false;
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe {
            self.reader.cancelReading();
            self.session.wait_for_asynchronous_frames();
            self.session.invalidate();
        }
    }
}

fn create_session(
    format: &CMFormatDescription,
    matrix: Matrix,
) -> Result<CFRetained<VTDecompressionSession>, DecodeError> {
    // SAFETY: These statics are constant CF objects defined by the frameworks.
    let (true_value, require_hardware, keys): (&CFType, _, _) = unsafe {
        (
            kCFBooleanTrue.expect("CoreFoundation defines true"),
            kVTVideoDecoderSpecification_RequireHardwareAcceleratedVideoDecoder,
            [
                kCVPixelBufferPixelFormatTypeKey,
                kCVPixelBufferIOSurfacePropertiesKey,
                kCVPixelBufferMetalCompatibilityKey,
            ],
        )
    };
    let specification =
        CFDictionary::<CFString, CFType>::from_slices(&[require_hardware], &[true_value]);
    let pixel_format = CFNumber::new_i32(PIXEL_FORMAT as i32);
    let no_surface_options = CFDictionary::<CFString, CFType>::empty();
    let attributes = CFDictionary::<CFString, CFType>::from_slices(
        &keys,
        &[&pixel_format, &no_surface_options, true_value],
    );
    let mut session = ptr::null_mut();
    // SAFETY: Both dictionaries hold CF values of the types VideoToolbox documents for these keys,
    // and a null callback record is allowed when decoding with an output handler.
    let status = unsafe {
        VTDecompressionSession::create(
            None,
            format,
            Some(specification.as_opaque()),
            Some(attributes.as_opaque()),
            ptr::null(),
            NonNull::from(&mut session),
        )
    };
    let session = match NonNull::new(session) {
        // SAFETY: Create returns an owned session on success.
        Some(session) if status == 0 => unsafe {
            CFRetained::<VTDecompressionSession>::from_raw(session)
        },
        _ => return Err(DecodeError::Session(status)),
    };
    if matrix == Matrix::Bt601 {
        // SAFETY: Constant CF strings defined by the frameworks.
        let (key, bt601) = unsafe {
            (
                kVTPixelTransferPropertyKey_DestinationYCbCrMatrix,
                kCVImageBufferYCbCrMatrix_ITU_R_601_4,
            )
        };
        let transfer = CFDictionary::<CFString, CFString>::from_slices(&[key], &[bt601]);
        // SAFETY: The property takes a dictionary of pixel transfer properties.
        let status = unsafe {
            VTSessionSetProperty(
                &session,
                kVTDecompressionPropertyKey_PixelTransferProperties,
                Some(transfer.as_opaque()),
            )
        };
        if status != 0 {
            return Err(DecodeError::PixelTransfer(status));
        }
    }
    Ok(session)
}

/// Starts a passthrough reader at `start`. Passthrough means no decode and no pixel format conversion.
fn start_reader(
    asset: &AVURLAsset,
    track: &AVAssetTrack,
    start: Duration,
) -> Result<(Retained<AVAssetReader>, Retained<AVAssetReaderTrackOutput>), DecodeError> {
    let reader = unsafe { AVAssetReader::assetReaderWithAsset_error(asset) }
        .map_err(|error| DecodeError::Reader(error.localizedDescription().to_string()))?;
    let output = unsafe {
        AVAssetReaderTrackOutput::assetReaderTrackOutputWithTrack_outputSettings(track, None)
    };
    unsafe {
        output.setAlwaysCopiesSampleData(false);
        reader.setTimeRange(CMTimeRange {
            start: to_cmtime(start),
            duration: kCMTimePositiveInfinity,
        });
        reader.addOutput(&output);
        if !reader.startReading() {
            return Err(reader_error(&reader));
        }
    }
    Ok((reader, output))
}

/// The next sample that carries media, skipping empty marker buffers.
fn next_sample(
    reader: &AVAssetReader,
    output: &AVAssetReaderTrackOutput,
) -> Result<Option<Retained<CMSampleBuffer>>, DecodeError> {
    loop {
        match unsafe { output.copyNextSampleBuffer() } {
            Some(sample) if unsafe { sample.num_samples() } > 0 => return Ok(Some(sample)),
            Some(_) => continue,
            None if unsafe { reader.status() } == AVAssetReaderStatus::Failed => {
                return Err(reader_error(reader));
            }
            None => return Ok(None),
        }
    }
}

fn reader_error(reader: &AVAssetReader) -> DecodeError {
    let message = unsafe { reader.error() }
        .map(|error| error.localizedDescription().to_string())
        .unwrap_or_else(|| "unknown reader error".into());
    DecodeError::Reader(message)
}

fn is_keyframe(sample: &CMSampleBuffer) -> bool {
    let Some(attachments) = (unsafe { sample.sample_attachments_array(false) }) else {
        return true;
    };
    // SAFETY: CoreMedia documents the array as one attachment dictionary per sample.
    let attachments = unsafe {
        CFRetained::cast_unchecked::<objc2_core_foundation::CFArray<CFDictionary<CFString, CFType>>>(
            attachments,
        )
    };
    let not_sync = attachments
        .get(0)
        .and_then(|dictionary| dictionary.get(unsafe { kCMSampleAttachmentKey_NotSync }))
        .and_then(|value| value.downcast::<CFBoolean>().ok())
        .is_some_and(|value| value.as_bool());
    !not_sync
}

fn to_duration(time: CMTime) -> Duration {
    let (value, timescale) = (time.value, time.timescale);
    if value <= 0 || timescale <= 0 {
        return Duration::ZERO;
    }
    Duration::from_nanos((value as u128 * 1_000_000_000 / timescale as u128) as u64)
}

fn to_cmtime(time: Duration) -> CMTime {
    unsafe { CMTime::new(time.as_nanos() as i64, 1_000_000_000) }
}
