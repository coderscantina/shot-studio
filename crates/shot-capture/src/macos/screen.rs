//! Display video and system audio through ScreenCaptureKit.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_core_graphics::{
    CGDisplayBounds, CGDisplayCopyDisplayMode, CGDisplayMode, CGMainDisplayID,
    CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess, kCGColorSpaceSRGB,
    kCGDisplayStreamYCbCrMatrix_ITU_R_709_2,
};
use objc2_core_media::{CMClock, CMSampleBuffer, CMTime};
use objc2_core_video::kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange;
use objc2_foundation::{NSArray, NSError};
use objc2_screen_capture_kit::{
    SCContentFilter, SCShareableContent, SCStream, SCStreamConfiguration, SCStreamOutputType,
};

use super::sink::SampleSink;
use super::tap::Latest;
use super::writer::{AUDIO_RATE, Track};
use crate::events::DisplayGeometry;
use crate::{CaptureError, Permission};

pub const FPS: u32 = 60;
const TIMEOUT: Duration = Duration::from_secs(10);

/// A running display capture.
///
/// ScreenCaptureKit only delivers a frame when the screen changes. A pacer thread writes the
/// newest frame on a fixed 60 fps grid instead, so `screen.mp4` is constant frame rate and gets
/// a keyframe and a fragment every 0.5 s even while the screen is idle.
pub struct Screen {
    stream: Retained<SCStream>,
    _sinks: Vec<Retained<SampleSink>>,
    _queues: Vec<DispatchRetained<DispatchQueue>>,
    pub geometry: DisplayGeometry,
    track: Arc<Track>,
    latest: Arc<Latest>,
    pacer: Option<(Arc<AtomicBool>, JoinHandle<()>)>,
}

/// Moves a ScreenCaptureKit object out of its completion handler.
struct Handoff<T>(T);
// SAFETY: the object is created on ScreenCaptureKit's queue and only used by the receiving thread
// after the handler returned.
unsafe impl<T> Send for Handoff<T> {}

pub fn check_permission() -> Result<(), CaptureError> {
    if CGPreflightScreenCaptureAccess() {
        return Ok(());
    }
    // Registers the app in System Settings and shows the prompt once.
    CGRequestScreenCaptureAccess();
    Err(CaptureError::Permission(Permission::ScreenRecording))
}

/// The display a session records: `id`, or the main display.
pub fn display_id(id: Option<u32>) -> u32 {
    id.unwrap_or_else(|| CGMainDisplayID())
}

/// Pixel size and refresh rate of a display's current mode.
pub fn pixel_mode(id: u32) -> Option<(usize, usize, f64)> {
    let mode = CGDisplayCopyDisplayMode(id)?;
    Some((
        CGDisplayMode::pixel_width(Some(&mode)),
        CGDisplayMode::pixel_height(Some(&mode)),
        CGDisplayMode::refresh_rate(Some(&mode)),
    ))
}

impl Screen {
    /// Starts capturing `display` into `screen`, and system audio into `system` if given.
    pub fn start(
        display: u32,
        screen: Arc<Track>,
        system: Option<Arc<Track>>,
        latest: Arc<Latest>,
    ) -> Result<Self, CaptureError> {
        let fail = |message: String| CaptureError::Source {
            what: "screen capture",
            message,
        };
        let content = shareable_content()?;
        let target = unsafe { content.displays() }
            .iter()
            .find(|d| unsafe { d.displayID() } == display)
            .ok_or(CaptureError::NoDisplay(display))?;
        let (width, height, _) = pixel_mode(display).ok_or(CaptureError::NoDisplay(display))?;
        let bounds = CGDisplayBounds(display);
        let geometry = DisplayGeometry {
            origin: (bounds.origin.x, bounds.origin.y),
            scale: width as f64 / bounds.size.width,
        };

        let filter = unsafe {
            SCContentFilter::initWithDisplay_excludingWindows(
                SCContentFilter::alloc(),
                &target,
                &NSArray::new(),
            )
        };
        let config = unsafe { SCStreamConfiguration::new() };
        unsafe {
            config.setWidth(width);
            config.setHeight(height);
            config.setMinimumFrameInterval(CMTime::new(1, FPS as i32));
            config.setPixelFormat(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange);
            config.setShowsCursor(false);
            config.setQueueDepth(8);
            config.setColorMatrix(kCGDisplayStreamYCbCrMatrix_ITU_R_709_2);
            config.setColorSpaceName(kCGColorSpaceSRGB);
            config.setCapturesAudio(system.is_some());
            config.setSampleRate(AUDIO_RATE as isize);
            config.setChannelCount(2);
            config.setExcludesCurrentProcessAudio(true);
        }
        let stream = unsafe {
            SCStream::initWithFilter_configuration_delegate(
                SCStream::alloc(),
                &filter,
                &config,
                None,
            )
        };

        let mut sinks = Vec::new();
        let mut queues = Vec::new();
        let video_sink = {
            let latest = latest.clone();
            SampleSink::new(Box::new(move |sample: &CMSampleBuffer| {
                // Samples without an image are status updates for an unchanged screen.
                if let Some(image) = unsafe { sample.image_buffer() } {
                    latest.store(image);
                }
            }))
        };
        let outputs = std::iter::once((SCStreamOutputType::Screen, screen.file, video_sink)).chain(
            system.map(|track| {
                let file = track.file;
                let sink =
                    SampleSink::new(Box::new(move |sample: &CMSampleBuffer| track.push(sample)));
                (SCStreamOutputType::Audio, file, sink)
            }),
        );
        for (kind, file, sink) in outputs {
            let queue = DispatchQueue::new(file, None);
            unsafe {
                stream.addStreamOutput_type_sampleHandlerQueue_error(
                    ProtocolObject::from_ref(&*sink),
                    kind,
                    Some(&queue),
                )
            }
            .map_err(|e| fail(e.localizedDescription().to_string()))?;
            sinks.push(sink);
            queues.push(queue);
        }

        wait_for(|handler| unsafe { stream.startCaptureWithCompletionHandler(Some(handler)) })
            .map_err(|e| match e {
                Some(_) if !CGPreflightScreenCaptureAccess() => {
                    CaptureError::Permission(Permission::ScreenRecording)
                }
                Some(message) => fail(message),
                None => fail("starting timed out".into()),
            })?;
        // Open the writer on the first frame now, so the pacer starts on schedule.
        let first_frame = Instant::now() + TIMEOUT;
        while Instant::now() < first_frame {
            if let Some(image) = latest.get() {
                screen.prepare(&image);
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(Self {
            stream,
            _sinks: sinks,
            _queues: queues,
            geometry,
            track: screen,
            latest,
            pacer: None,
        })
    }

    /// The clock ScreenCaptureKit stamps samples with.
    pub fn clock(&self) -> Option<Retained<CMClock>> {
        unsafe { self.stream.synchronizationClock() }
    }

    /// Starts writing frames, frame `k` at media time `k / FPS` after `origin`.
    pub fn pace(&mut self, origin: Instant) {
        let stop = Arc::new(AtomicBool::new(false));
        let thread = std::thread::Builder::new()
            .name("screen.mp4 pacer".into())
            .spawn({
                let (stop, track, latest) = (stop.clone(), self.track.clone(), self.latest.clone());
                move || {
                    let tick = |k: u64| Duration::from_nanos(k * 1_000_000_000 / u64::from(FPS));
                    let duration = unsafe { CMTime::new(1, FPS as i32) };
                    let mut k = 0;
                    while !stop.load(Ordering::Relaxed) {
                        let elapsed = origin.elapsed();
                        if elapsed < tick(k) {
                            std::thread::sleep(tick(k) - elapsed);
                        } else if elapsed > tick(k + u64::from(FPS)) {
                            // A short stall catches up frame by frame.
                            // After a long one, skip to the current slot instead of bursting.
                            k = elapsed.as_nanos() as u64 * u64::from(FPS) / 1_000_000_000;
                        }
                        let image = latest.get();
                        if let Some(image) = image {
                            let at = unsafe { CMTime::new(k as i64, FPS as i32) };
                            track.push_frame(&image, at, duration);
                        }
                        k += 1;
                    }
                }
            })
            .expect("spawning the pacer");
        self.pacer = Some((stop, thread));
    }

    pub fn stop(&mut self) {
        let _ = wait_for(|handler| unsafe {
            self.stream.stopCaptureWithCompletionHandler(Some(handler))
        });
        if let Some((stop, thread)) = self.pacer.take() {
            stop.store(true, Ordering::Relaxed);
            let _ = thread.join();
        }
    }
}

/// Runs an async ScreenCaptureKit call and waits for its completion handler. `Err(None)` is a
/// timeout.
fn wait_for(
    call: impl FnOnce(&block2::DynBlock<dyn Fn(*mut NSError)>),
) -> Result<(), Option<String>> {
    let (done, wait) = mpsc::channel();
    let handler = RcBlock::new(move |error: *mut NSError| {
        let message = unsafe { error.as_ref() }.map(|e| e.localizedDescription().to_string());
        let _ = done.send(message);
    });
    call(&handler);
    match wait.recv_timeout(TIMEOUT) {
        Ok(None) => Ok(()),
        Ok(Some(message)) => Err(Some(message)),
        Err(_) => Err(None),
    }
}

fn shareable_content() -> Result<Retained<SCShareableContent>, CaptureError> {
    let (done, wait) = mpsc::channel();
    let handler = RcBlock::new(
        move |content: *mut SCShareableContent, error: *mut NSError| {
            let result = match unsafe { Retained::retain(content) } {
                Some(content) => Ok(Handoff(content)),
                None => Err(unsafe { error.as_ref() }
                    .map(|e| e.localizedDescription().to_string())
                    .unwrap_or_default()),
            };
            let _ = done.send(result);
        },
    );
    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&handler) };
    match wait.recv_timeout(TIMEOUT) {
        Ok(Ok(Handoff(content))) => Ok(content),
        Ok(Err(_)) if !CGPreflightScreenCaptureAccess() => {
            Err(CaptureError::Permission(Permission::ScreenRecording))
        }
        Ok(Err(message)) => Err(CaptureError::Source {
            what: "listing displays",
            message,
        }),
        Err(_) => Err(CaptureError::Source {
            what: "listing displays",
            message: "timed out".into(),
        }),
    }
}
