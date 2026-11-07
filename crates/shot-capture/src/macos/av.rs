//! Camera and mic through one AVCaptureSession.

use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, ProtocolObject};
use objc2_av_foundation::{
    AVAuthorizationStatus, AVCaptureAudioDataOutput, AVCaptureDevice,
    AVCaptureDeviceDiscoverySession, AVCaptureDeviceInput, AVCaptureDevicePosition,
    AVCaptureDeviceTypeBuiltInWideAngleCamera, AVCaptureDeviceTypeContinuityCamera,
    AVCaptureDeviceTypeExternal, AVCaptureDeviceTypeMicrophone, AVCaptureSession,
    AVCaptureSessionPreset1920x1080, AVCaptureSessionPresetHigh, AVCaptureVideoDataOutput,
    AVMediaType, AVMediaTypeAudio, AVMediaTypeVideo,
};
use objc2_avf_audio::{
    AVFormatIDKey, AVLinearPCMBitDepthKey, AVLinearPCMIsBigEndianKey, AVLinearPCMIsFloatKey,
    AVLinearPCMIsNonInterleaved, AVNumberOfChannelsKey, AVSampleRateKey,
};
use objc2_core_audio_types::{
    kAudioFormatFlagIsFloat, kAudioFormatFlagIsNonInterleaved, kAudioFormatLinearPCM,
};
use objc2_core_foundation::CFString;
use objc2_core_media::{
    CMAudioFormatDescriptionGetStreamBasicDescription, CMClock, CMSampleBuffer,
};
use objc2_core_video::{
    kCVPixelBufferPixelFormatTypeKey, kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
};
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString};

use super::sink::SampleSink;
use super::tap::Latest;
use super::writer::{AUDIO_RATE, Track};
use crate::clock::nanos;
use crate::{CaptureError, Device, MicTap, Permission, Source};

const PERMISSION_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Camera,
    Mic,
}

impl Kind {
    fn media_type(self) -> &'static AVMediaType {
        match self {
            Self::Camera => unsafe { AVMediaTypeVideo },
            Self::Mic => unsafe { AVMediaTypeAudio },
        }
        .expect("AVFoundation defines the media types")
    }

    fn permission(self) -> Permission {
        match self {
            Self::Camera => Permission::Camera,
            Self::Mic => Permission::Microphone,
        }
    }

    fn noun(self) -> &'static str {
        match self {
            Self::Camera => "camera",
            Self::Mic => "mic",
        }
    }
}

/// Every camera or mic AVFoundation offers.
pub fn devices(kind: Kind) -> Vec<Retained<AVCaptureDevice>> {
    let types = unsafe {
        match kind {
            Kind::Camera => NSArray::from_slice(&[
                AVCaptureDeviceTypeBuiltInWideAngleCamera,
                AVCaptureDeviceTypeContinuityCamera,
                AVCaptureDeviceTypeExternal,
            ]),
            Kind::Mic => NSArray::from_slice(&[AVCaptureDeviceTypeMicrophone]),
        }
    };
    let session = unsafe {
        AVCaptureDeviceDiscoverySession::discoverySessionWithDeviceTypes_mediaType_position(
            &types,
            Some(kind.media_type()),
            AVCaptureDevicePosition::Unspecified,
        )
    };
    unsafe { session.devices() }.to_vec()
}

pub fn describe(kind: Kind) -> Vec<Device> {
    let default = default_device(kind).map(|d| unsafe { d.uniqueID() }.to_string());
    devices(kind)
        .iter()
        .map(|d| {
            let unique_id = unsafe { d.uniqueID() }.to_string();
            Device {
                name: unsafe { d.localizedName() }.to_string(),
                default: default.as_deref() == Some(&unique_id),
                unique_id,
            }
        })
        .collect()
}

fn default_device(kind: Kind) -> Option<Retained<AVCaptureDevice>> {
    unsafe { AVCaptureDevice::defaultDeviceWithMediaType(kind.media_type()) }
}

/// The device a source names, `None` when it is off or when there is no default device.
pub fn resolve(
    kind: Kind,
    source: &Source,
) -> Result<Option<Retained<AVCaptureDevice>>, CaptureError> {
    match source {
        Source::Off => Ok(None),
        Source::Default => Ok(default_device(kind)),
        Source::Named(name) => devices(kind)
            .into_iter()
            .find(|d| unsafe {
                d.localizedName().to_string().eq_ignore_ascii_case(name)
                    || d.uniqueID().to_string() == *name
            })
            .map(Some)
            .ok_or_else(|| CaptureError::NoDevice {
                kind: kind.noun(),
                name: name.clone(),
            }),
    }
}

/// Asks for camera or mic access when macOS has not asked yet, and fails when it is denied.
pub fn authorize(kind: Kind) -> Result<(), CaptureError> {
    let denied = Err(CaptureError::Permission(kind.permission()));
    match unsafe { AVCaptureDevice::authorizationStatusForMediaType(kind.media_type()) } {
        AVAuthorizationStatus::Authorized => Ok(()),
        AVAuthorizationStatus::NotDetermined => {
            let (done, wait) = mpsc::channel();
            let handler = RcBlock::new(move |granted: Bool| {
                let _ = done.send(granted.as_bool());
            });
            unsafe {
                AVCaptureDevice::requestAccessForMediaType_completionHandler(
                    kind.media_type(),
                    &handler,
                )
            };
            match wait.recv_timeout(PERMISSION_TIMEOUT) {
                Ok(true) => Ok(()),
                _ => denied,
            }
        }
        _ => denied,
    }
}

/// A running camera and mic capture.
pub struct AvCapture {
    session: Retained<AVCaptureSession>,
    _sinks: Vec<Retained<SampleSink>>,
    _queues: Vec<DispatchRetained<DispatchQueue>>,
}

impl AvCapture {
    pub fn start(
        camera: Option<(Retained<AVCaptureDevice>, Arc<Track>, Arc<Latest>)>,
        mic: Option<(Retained<AVCaptureDevice>, Arc<Track>, Option<MicTap>)>,
    ) -> Result<Self, CaptureError> {
        let fail = |message: String| CaptureError::Source {
            what: "camera and mic capture",
            message,
        };
        let session = unsafe { AVCaptureSession::new() };
        let mut sinks = Vec::new();
        let mut queues = Vec::new();
        unsafe { session.beginConfiguration() };

        if let Some((device, track, latest)) = camera {
            let preset = unsafe { AVCaptureSessionPreset1920x1080 };
            unsafe {
                session.setSessionPreset(if session.canSetSessionPreset(preset) {
                    preset
                } else {
                    AVCaptureSessionPresetHigh
                })
            };
            add_input(&session, &device).map_err(fail)?;
            let output = unsafe { AVCaptureVideoDataOutput::new() };
            let pixel_format = NSNumber::new_u32(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange);
            let settings = NSDictionary::<NSString, AnyObject>::from_slices(
                &[ns(unsafe { kCVPixelBufferPixelFormatTypeKey })],
                &[&*pixel_format],
            );
            let sink = SampleSink::new(Box::new(move |sample: &CMSampleBuffer| {
                if let Some(image) = unsafe { sample.image_buffer() } {
                    latest.store(image);
                }
                track.push(sample)
            }));
            let queue = DispatchQueue::new("camera.mp4", None);
            unsafe {
                output.setVideoSettings(Some(&settings));
                output.setAlwaysDiscardsLateVideoFrames(true);
                output.setSampleBufferDelegate_queue(
                    Some(ProtocolObject::from_ref(&*sink)),
                    Some(&queue),
                );
                if !session.canAddOutput(&output) {
                    return Err(fail("the camera output was refused".into()));
                }
                session.addOutput(&output);
            }
            sinks.push(sink);
            queues.push(queue);
        }

        if let Some((device, track, tap)) = mic {
            add_input(&session, &device).map_err(fail)?;
            let output = unsafe { AVCaptureAudioDataOutput::new() };
            let settings = unsafe {
                let key = |k: Option<&'static NSString>| k.expect("AVFAudio defines its keys");
                NSDictionary::<NSString, AnyObject>::from_slices(
                    &[
                        key(AVFormatIDKey),
                        key(AVSampleRateKey),
                        key(AVNumberOfChannelsKey),
                        key(AVLinearPCMBitDepthKey),
                        key(AVLinearPCMIsFloatKey),
                        key(AVLinearPCMIsNonInterleaved),
                        key(AVLinearPCMIsBigEndianKey),
                    ],
                    &[
                        &*NSNumber::new_u32(kAudioFormatLinearPCM),
                        &*NSNumber::new_f64(AUDIO_RATE),
                        &*NSNumber::new_u32(1),
                        &*NSNumber::new_u32(32),
                        &*NSNumber::new_bool(true),
                        &*NSNumber::new_bool(false),
                        &*NSNumber::new_bool(false),
                    ],
                )
            };
            let tap = tap.map(Mutex::new);
            let sink = SampleSink::new(Box::new(move |sample: &CMSampleBuffer| {
                track.push(sample);
                if let (Some(tap), Some(clock)) = (&tap, track.clock()) {
                    let pts = unsafe { sample.presentation_time_stamp() };
                    if let (Some(at), Some((samples, rate))) = (
                        clock.media_time(nanos(pts.value, pts.timescale)),
                        mono(sample),
                    ) {
                        (tap.lock().expect("mic tap poisoned"))(at, &samples, rate);
                    }
                }
            }));
            let queue = DispatchQueue::new("mic.mp4", None);
            unsafe {
                output.setAudioSettings(Some(&settings));
                output.setSampleBufferDelegate_queue(
                    Some(ProtocolObject::from_ref(&*sink)),
                    Some(&queue),
                );
                if !session.canAddOutput(&output) {
                    return Err(fail("the mic output was refused".into()));
                }
                session.addOutput(&output);
            }
            sinks.push(sink);
            queues.push(queue);
        }

        unsafe {
            session.commitConfiguration();
            session.startRunning();
            if !session.isRunning() {
                return Err(fail("the capture session did not start".into()));
            }
        }
        Ok(Self {
            session,
            _sinks: sinks,
            _queues: queues,
        })
    }

    /// The clock the session stamps samples with.
    pub fn clock(&self) -> Option<Retained<CMClock>> {
        unsafe { self.session.synchronizationClock() }
    }

    pub fn stop(&self) {
        unsafe { self.session.stopRunning() };
    }
}

fn add_input(session: &AVCaptureSession, device: &AVCaptureDevice) -> Result<(), String> {
    let input = unsafe { AVCaptureDeviceInput::deviceInputWithDevice_error(device) }
        .map_err(|e| e.localizedDescription().to_string())?;
    unsafe {
        if !session.canAddInput(&input) {
            return Err(format!("{} cannot be added", device.localizedName()));
        }
        session.addInput(&input);
    }
    Ok(())
}

fn ns(s: &CFString) -> &NSString {
    // SAFETY: CFString and NSString are toll-free bridged.
    unsafe { &*(s as *const CFString).cast::<NSString>() }
}

/// The samples of a float PCM buffer as mono, and their sample rate. Interleaved channels are
/// averaged, of planar ones the first is used.
fn mono(sample: &CMSampleBuffer) -> Option<(Vec<f32>, u32)> {
    let format = unsafe { sample.format_description() }?;
    let asbd = unsafe { CMAudioFormatDescriptionGetStreamBasicDescription(&format).as_ref() }?;
    if asbd.mFormatFlags & kAudioFormatFlagIsFloat == 0 || asbd.mBitsPerChannel != 32 {
        return None;
    }
    let frames = usize::try_from(unsafe { sample.num_samples() }).ok()?;
    let data = unsafe { sample.data_buffer() }?;
    let len = unsafe { data.data_length() };
    let mut all = vec![0f32; len / 4];
    let status = unsafe {
        data.copy_data_bytes(
            0,
            all.len() * 4,
            std::ptr::NonNull::from(&mut all[..]).cast(),
        )
    };
    if status != 0 {
        return None;
    }
    let channels = asbd.mChannelsPerFrame.max(1) as usize;
    let samples = if channels == 1 || asbd.mFormatFlags & kAudioFormatFlagIsNonInterleaved != 0 {
        all.truncate(frames);
        all
    } else {
        all.chunks_exact(channels)
            .map(|frame| frame.iter().sum::<f32>() / channels as f32)
            .collect()
    };
    Some((samples, asbd.mSampleRate as u32))
}
