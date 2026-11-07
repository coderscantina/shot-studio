//! Objective-C delegates: one forwards sample buffers from ScreenCaptureKit and AVCaptureSession
//! outputs to a Rust closure, the other appends an asset writer's segments to a file.

use std::fs::File;
use std::io::Write;
use std::sync::Mutex;

use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol};
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
use objc2_av_foundation::{
    AVAssetSegmentType, AVAssetWriter, AVAssetWriterDelegate,
    AVCaptureAudioDataOutputSampleBufferDelegate, AVCaptureConnection, AVCaptureOutput,
    AVCaptureVideoDataOutputSampleBufferDelegate,
};
use objc2_core_media::CMSampleBuffer;
use objc2_foundation::NSData;
use objc2_screen_capture_kit::{SCStream, SCStreamOutput, SCStreamOutputType};

pub type Handler = Box<dyn Fn(&CMSampleBuffer) + Send + Sync>;

define_class!(
    // SAFETY: NSObject has no subclassing requirements and SampleSink does not implement Drop.
    #[unsafe(super(NSObject))]
    #[ivars = Handler]
    pub struct SampleSink;

    impl SampleSink {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn stream_output(&self, _stream: &SCStream, sample: &CMSampleBuffer, _kind: SCStreamOutputType) {
            (self.ivars())(sample);
        }

        #[unsafe(method(captureOutput:didOutputSampleBuffer:fromConnection:))]
        fn capture_output(
            &self,
            _output: &AVCaptureOutput,
            sample: &CMSampleBuffer,
            _connection: &AVCaptureConnection,
        ) {
            (self.ivars())(sample);
        }
    }

    unsafe impl NSObjectProtocol for SampleSink {}
    unsafe impl SCStreamOutput for SampleSink {}
    unsafe impl AVCaptureVideoDataOutputSampleBufferDelegate for SampleSink {}
    unsafe impl AVCaptureAudioDataOutputSampleBufferDelegate for SampleSink {}
);

impl SampleSink {
    pub fn new(handler: Handler) -> Retained<Self> {
        let this = Self::alloc().set_ivars(handler);
        unsafe { msg_send![super(this), init] }
    }
}

/// The open file, or why writing to it failed.
pub type SegmentFile = Mutex<Result<File, String>>;

define_class!(
    // SAFETY: NSObject has no subclassing requirements and SegmentSink does not implement Drop.
    #[unsafe(super(NSObject))]
    #[ivars = SegmentFile]
    pub struct SegmentSink;

    impl SegmentSink {
        #[unsafe(method(assetWriter:didOutputSegmentData:segmentType:))]
        fn segment(&self, _writer: &AVAssetWriter, data: &NSData, _kind: AVAssetSegmentType) {
            let mut file = self.ivars().lock().expect("segment file poisoned");
            // SAFETY: the writer does not mutate the segment while the delegate runs.
            let bytes = unsafe { data.as_bytes_unchecked() };
            if let Err(e) = file.as_mut().map(|f| f.write_all(bytes)) .unwrap_or(Ok(())) {
                *file = Err(e.to_string());
            }
        }
    }

    unsafe impl NSObjectProtocol for SegmentSink {}
    unsafe impl AVAssetWriterDelegate for SegmentSink {}
);

impl SegmentSink {
    /// Appends every segment, the initialization segment first, to `file`.
    pub fn new(file: File) -> Retained<Self> {
        let this = Self::alloc().set_ivars(Mutex::new(Ok(file)));
        unsafe { msg_send![super(this), init] }
    }

    /// Why writing failed, if it did.
    pub fn error(&self) -> Option<String> {
        self.ivars()
            .lock()
            .expect("segment file poisoned")
            .as_ref()
            .err()
            .cloned()
    }
}
