//! Hardware HEVC encode: frames go in as IOSurface-backed `CVPixelBuffer`s from the session's own
//! pool, so the encoder reads them in place, and come out as length-prefixed HEVC samples ready
//! for an MP4 `hvc1` track.

use std::collections::VecDeque;
use std::ptr::{self, NonNull};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use block2::RcBlock;
use objc2_core_foundation::{
    CFBoolean, CFData, CFDictionary, CFNumber, CFRetained, CFString, CFType,
};
use objc2_core_media::{
    CMSampleBuffer, CMTime, kCMFormatDescriptionExtension_SampleDescriptionExtensionAtoms,
    kCMSampleAttachmentKey_NotSync, kCMTimeInvalid, kCMVideoCodecType_HEVC,
};
use objc2_core_video::{
    CVImageBuffer, CVPixelBuffer, CVPixelBufferPool, kCVImageBufferColorPrimaries_ITU_R_709_2,
    kCVImageBufferTransferFunction_ITU_R_709_2, kCVImageBufferYCbCrMatrix_ITU_R_709_2,
    kCVPixelBufferHeightKey, kCVPixelBufferIOSurfacePropertiesKey,
    kCVPixelBufferMetalCompatibilityKey, kCVPixelBufferPixelFormatTypeKey, kCVPixelBufferWidthKey,
    kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
};
use objc2_video_toolbox::{
    VTCompressionSession, VTEncodeInfoFlags, VTSessionSetProperty,
    kVTCompressionPropertyKey_AllowFrameReordering, kVTCompressionPropertyKey_ColorPrimaries,
    kVTCompressionPropertyKey_ExpectedFrameRate, kVTCompressionPropertyKey_MaxKeyFrameInterval,
    kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration,
    kVTCompressionPropertyKey_PrioritizeEncodingSpeedOverQuality,
    kVTCompressionPropertyKey_ProfileLevel, kVTCompressionPropertyKey_Quality,
    kVTCompressionPropertyKey_RealTime, kVTCompressionPropertyKey_TransferFunction,
    kVTCompressionPropertyKey_YCbCrMatrix, kVTEncodeFrameOptionKey_ForceKeyFrame,
    kVTProfileLevel_HEVC_Main_AutoLevel,
    kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder,
};

/// The encoder's input format: video range biplanar 4:2:0, what HEVC Main stores.
pub const INPUT_PIXEL_FORMAT: u32 = kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange;

/// Frames submitted but not yet returned. Bounds memory: each 4K input buffer is 12 MB.
const MAX_IN_FLIGHT: usize = 6;

#[derive(Debug, thiserror::Error)]
pub enum EncodeError {
    #[error("creating the hardware HEVC encoder failed with OSStatus {0}")]
    Session(i32),
    #[error("setting encoder property {0} failed with OSStatus {1}")]
    Property(&'static str, i32),
    #[error("the encoder has no input buffer pool")]
    NoPool,
    #[error("allocating an encoder input buffer failed with OSStatus {0}")]
    Buffer(i32),
    #[error("encoding a frame failed with OSStatus {0}")]
    Encode(i32),
    #[error("the encoder dropped a frame")]
    Dropped,
    #[error("the encoder output has no HEVC parameter sets")]
    NoParameterSets,
}

/// Fixed settings of one encode session.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EncoderSettings {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// Frames from one keyframe to the next.
    pub keyframe_interval: u32,
    /// VideoToolbox quality from 0 to 1. The encoder spends bits until it reaches it, with no
    /// bitrate cap.
    pub quality: f32,
}

/// One compressed frame in output order, as length-prefixed NAL units.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedFrame {
    /// The frame's index in submission order.
    pub index: u64,
    pub keyframe: bool,
    pub data: Vec<u8>,
}

#[derive(Default)]
struct Outputs {
    done: VecDeque<Result<EncodedFrame, EncodeError>>,
    pending: usize,
    hvcc: Option<Vec<u8>>,
}

#[derive(Default)]
struct Shared {
    outputs: Mutex<Outputs>,
    changed: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Outputs> {
        self.outputs.lock().expect("encoder output poisoned")
    }
}

type OutputHandler = RcBlock<dyn Fn(i32, VTEncodeInfoFlags, *mut CMSampleBuffer)>;

/// A `VTCompressionSession` on the hardware HEVC encoder, without frame reordering.
pub struct HevcEncoder {
    session: CFRetained<VTCompressionSession>,
    pool: CFRetained<CVPixelBufferPool>,
    shared: Arc<Shared>,
    handler: OutputHandler,
    fps: i32,
    submitted: u64,
}

impl HevcEncoder {
    pub fn new(settings: EncoderSettings) -> Result<Self, EncodeError> {
        let session = create_session(settings)?;
        configure(&session, settings)?;
        let pool = unsafe { session.pixel_buffer_pool() }.ok_or(EncodeError::NoPool)?;
        let shared = Arc::new(Shared::default());
        let handler = {
            let shared = shared.clone();
            RcBlock::new(
                move |status: i32, _flags: VTEncodeInfoFlags, sample: *mut CMSampleBuffer| {
                    let mut outputs = shared.lock();
                    let result = match NonNull::new(sample) {
                        Some(sample) if status == 0 => {
                            // SAFETY: VideoToolbox passes a valid sample for the duration of the call.
                            let sample = unsafe { sample.as_ref() };
                            if outputs.hvcc.is_none() {
                                outputs.hvcc = hvcc(sample);
                            }
                            Ok(encoded_frame(sample))
                        }
                        Some(_) => Err(EncodeError::Encode(status)),
                        None if status != 0 => Err(EncodeError::Encode(status)),
                        None => Err(EncodeError::Dropped),
                    };
                    outputs.done.push_back(result);
                    outputs.pending -= 1;
                    shared.changed.notify_all();
                },
            )
        };
        Ok(Self {
            session,
            pool,
            shared,
            handler,
            fps: settings.fps as i32,
            submitted: 0,
        })
    }

    /// An empty input frame from the encoder's pool: IOSurface-backed, Metal compatible,
    /// [`INPUT_PIXEL_FORMAT`].
    pub fn input_buffer(&self) -> Result<CFRetained<CVPixelBuffer>, EncodeError> {
        let mut buffer = ptr::null_mut();
        // SAFETY: The pool is live and the out pointer is writable.
        let status = unsafe {
            CVPixelBufferPool::create_pixel_buffer(None, &self.pool, NonNull::from(&mut buffer))
        };
        match NonNull::new(buffer) {
            // SAFETY: Create returns an owned buffer on success.
            Some(buffer) if status == 0 => Ok(unsafe { CFRetained::from_raw(buffer) }),
            _ => Err(EncodeError::Buffer(status)),
        }
    }

    /// Submits the next frame. Blocks while too many frames are in flight.
    pub fn encode(&mut self, buffer: &CVPixelBuffer) -> Result<(), EncodeError> {
        self.submit(buffer, false)
    }

    /// Like [`Self::encode`], and the frame becomes a keyframe, so the stream can be cut before it.
    pub fn encode_keyframe(&mut self, buffer: &CVPixelBuffer) -> Result<(), EncodeError> {
        self.submit(buffer, true)
    }

    fn submit(&mut self, buffer: &CVPixelBuffer, keyframe: bool) -> Result<(), EncodeError> {
        {
            let mut outputs = self.shared.lock();
            while outputs.pending >= MAX_IN_FLIGHT {
                outputs = self
                    .shared
                    .changed
                    .wait(outputs)
                    .expect("encoder output poisoned");
            }
            outputs.pending += 1;
        }
        let pts = unsafe { CMTime::new(self.submitted as i64, self.fps) };
        let duration = unsafe { CMTime::new(1, self.fps) };
        let image: &CVImageBuffer = buffer;
        let force: &CFType = CFBoolean::new(true);
        // SAFETY: A constant CF string defined by the framework.
        let properties = keyframe.then(|| {
            CFDictionary::<CFString, CFType>::from_slices(
                &[unsafe { kVTEncodeFrameOptionKey_ForceKeyFrame }],
                &[force],
            )
        });
        // SAFETY: The buffer comes from this session's pool, the frame properties map a
        // documented key to a CFBoolean, and the session copies the handler block for each frame.
        let status = unsafe {
            self.session.encode_frame_with_output_handler(
                image,
                pts,
                duration,
                properties.as_deref().map(|p| p.as_opaque()),
                ptr::null_mut(),
                RcBlock::as_ptr(&self.handler),
            )
        };
        if status != 0 {
            self.shared.lock().pending -= 1;
            return Err(EncodeError::Encode(status));
        }
        self.submitted += 1;
        Ok(())
    }

    /// Frames the encoder has finished since the last call, in order.
    pub fn take_encoded(&mut self) -> Result<Vec<EncodedFrame>, EncodeError> {
        self.shared.lock().done.drain(..).collect()
    }

    /// Waits for every submitted frame and returns the ones not taken yet.
    pub fn finish(&mut self) -> Result<Vec<EncodedFrame>, EncodeError> {
        let status = unsafe { self.session.complete_frames(kCMTimeInvalid) };
        if status != 0 {
            return Err(EncodeError::Encode(status));
        }
        self.take_encoded()
    }

    /// The `hvcC` record of the encoded stream, known once the first frame came out.
    pub fn hvcc(&self) -> Result<Vec<u8>, EncodeError> {
        self.shared
            .lock()
            .hvcc
            .clone()
            .ok_or(EncodeError::NoParameterSets)
    }
}

impl Drop for HevcEncoder {
    fn drop(&mut self) {
        unsafe {
            self.session.complete_frames(kCMTimeInvalid);
            self.session.invalidate();
        }
    }
}

fn create_session(
    settings: EncoderSettings,
) -> Result<CFRetained<VTCompressionSession>, EncodeError> {
    let true_value: &CFType = CFBoolean::new(true);
    // SAFETY: These statics are constant CF strings defined by the frameworks.
    let specification = CFDictionary::<CFString, CFType>::from_slices(
        &[unsafe { kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder }],
        &[true_value],
    );
    let format = CFNumber::new_i32(INPUT_PIXEL_FORMAT as i32);
    let width = CFNumber::new_i32(settings.width as i32);
    let height = CFNumber::new_i32(settings.height as i32);
    let no_surface_options = CFDictionary::<CFString, CFType>::empty();
    let attributes = unsafe {
        CFDictionary::<CFString, CFType>::from_slices(
            &[
                kCVPixelBufferPixelFormatTypeKey,
                kCVPixelBufferWidthKey,
                kCVPixelBufferHeightKey,
                kCVPixelBufferIOSurfacePropertiesKey,
                kCVPixelBufferMetalCompatibilityKey,
            ],
            &[&format, &width, &height, &no_surface_options, true_value],
        )
    };
    let mut session = ptr::null_mut();
    // SAFETY: Both dictionaries hold the value types VideoToolbox documents for their keys, and a
    // null callback is allowed when every frame is encoded with an output handler.
    let status = unsafe {
        VTCompressionSession::create(
            None,
            settings.width as i32,
            settings.height as i32,
            kCMVideoCodecType_HEVC,
            Some(specification.as_opaque()),
            Some(attributes.as_opaque()),
            None,
            None,
            ptr::null_mut(),
            NonNull::from(&mut session),
        )
    };
    match NonNull::new(session) {
        // SAFETY: Create returns an owned session on success.
        Some(session) if status == 0 => Ok(unsafe { CFRetained::from_raw(session) }),
        _ => Err(EncodeError::Session(status)),
    }
}

fn configure(session: &VTCompressionSession, settings: EncoderSettings) -> Result<(), EncodeError> {
    let no: &CFType = CFBoolean::new(false);
    let interval = CFNumber::new_i32(settings.keyframe_interval as i32);
    let interval_seconds =
        CFNumber::new_f64(f64::from(settings.keyframe_interval) / f64::from(settings.fps));
    let quality = CFNumber::new_f32(settings.quality);
    let fps = CFNumber::new_i32(settings.fps as i32);
    // SAFETY: Constant CF strings from the frameworks.
    let yes: &CFType = CFBoolean::new(true);
    let properties: [(&str, &CFString, &CFType); 11] = unsafe {
        [
            ("RealTime", kVTCompressionPropertyKey_RealTime, no),
            // Doubles throughput on the M1 Pro (49 to 92 fps at 4K) for about 0.5 dB PSNR.
            (
                "PrioritizeEncodingSpeedOverQuality",
                kVTCompressionPropertyKey_PrioritizeEncodingSpeedOverQuality,
                yes,
            ),
            (
                "ProfileLevel",
                kVTCompressionPropertyKey_ProfileLevel,
                kVTProfileLevel_HEVC_Main_AutoLevel,
            ),
            (
                "AllowFrameReordering",
                kVTCompressionPropertyKey_AllowFrameReordering,
                no,
            ),
            (
                "MaxKeyFrameInterval",
                kVTCompressionPropertyKey_MaxKeyFrameInterval,
                &interval,
            ),
            (
                "MaxKeyFrameIntervalDuration",
                kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration,
                &interval_seconds,
            ),
            ("Quality", kVTCompressionPropertyKey_Quality, &quality),
            (
                "ExpectedFrameRate",
                kVTCompressionPropertyKey_ExpectedFrameRate,
                &fps,
            ),
            (
                "ColorPrimaries",
                kVTCompressionPropertyKey_ColorPrimaries,
                kCVImageBufferColorPrimaries_ITU_R_709_2,
            ),
            (
                "TransferFunction",
                kVTCompressionPropertyKey_TransferFunction,
                kCVImageBufferTransferFunction_ITU_R_709_2,
            ),
            (
                "YCbCrMatrix",
                kVTCompressionPropertyKey_YCbCrMatrix,
                kCVImageBufferYCbCrMatrix_ITU_R_709_2,
            ),
        ]
    };
    for (name, key, value) in properties {
        // SAFETY: Each value has the CF type VideoToolbox documents for its key.
        let status = unsafe { VTSessionSetProperty(session, key, Some(value)) };
        if status != 0 {
            return Err(EncodeError::Property(name, status));
        }
    }
    let status = unsafe { session.prepare_to_encode_frames() };
    if status != 0 {
        return Err(EncodeError::Session(status));
    }
    Ok(())
}

fn encoded_frame(sample: &CMSampleBuffer) -> EncodedFrame {
    let pts = unsafe { sample.presentation_time_stamp() };
    let index = if pts.timescale > 0 {
        pts.value as u64
    } else {
        0
    };
    let data = unsafe { sample.data_buffer() }
        .map(|block| {
            let length = unsafe { block.data_length() };
            let mut data = vec![0u8; length];
            // SAFETY: The destination holds exactly `length` bytes. This copies the compressed
            // bitstream, about 100 KB per 4K frame, never pixels.
            unsafe {
                block.copy_data_bytes(0, length, NonNull::from(&mut data[..]).cast());
            }
            data
        })
        .unwrap_or_default();
    EncodedFrame {
        index,
        keyframe: is_keyframe(sample),
        data,
    }
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
    !attachments
        .get(0)
        .and_then(|dictionary| dictionary.get(unsafe { kCMSampleAttachmentKey_NotSync }))
        .and_then(|value| value.downcast::<CFBoolean>().ok())
        .is_some_and(|value| value.as_bool())
}

/// The `hvcC` atom VideoToolbox attaches to the sample's format description.
fn hvcc(sample: &CMSampleBuffer) -> Option<Vec<u8>> {
    let format = unsafe { sample.format_description() }?;
    let atoms =
        unsafe { format.extension(kCMFormatDescriptionExtension_SampleDescriptionExtensionAtoms) }?;
    let atoms = atoms.downcast::<CFDictionary>().ok()?;
    // SAFETY: CoreMedia documents the extension as a dictionary of atom names to CFData.
    let atoms = unsafe { CFRetained::cast_unchecked::<CFDictionary<CFString, CFType>>(atoms) };
    let data = atoms.get(&CFString::from_str("hvcC"))?;
    let data = data.downcast::<CFData>().ok()?;
    Some(data.to_vec())
}
