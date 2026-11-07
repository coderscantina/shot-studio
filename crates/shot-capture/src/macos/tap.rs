//! The frame tap behind [`Preview`]: the newest raw frame of each video source, converted in
//! hardware on a thread of its own for a live preview.
//!
//! The capture queues only swap a pointer into [`Latest`]. Converting and scaling happens off
//! the queues, one frame at a time from the newest one, so a slow preview skips frames and never
//! holds up the recording.

use std::ptr::{self, NonNull};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use objc2_core_foundation::{
    CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType, kCFBooleanTrue,
};
use objc2_core_video::{
    CVImageBuffer, CVPixelBuffer, CVPixelBufferGetHeight, CVPixelBufferGetWidth, CVPixelBufferPool,
    kCVImageBufferYCbCrMatrix_ITU_R_601_4, kCVPixelBufferHeightKey,
    kCVPixelBufferIOSurfacePropertiesKey, kCVPixelBufferMetalCompatibilityKey,
    kCVPixelBufferPixelFormatTypeKey, kCVPixelBufferWidthKey,
    kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
};
use objc2_video_toolbox::{
    VTPixelTransferSession, VTSessionSetProperty,
    kVTPixelTransferPropertyKey_DestinationYCbCrMatrix, kVTPixelTransferPropertyKey_RealTime,
};

const SCREEN_MAX: (usize, usize) = (1920, 1080);
const CAMERA_MAX: (usize, usize) = (640, 360);
const REFRESH: Duration = Duration::from_micros(16_667);

/// The newest raw frame of a source, with a counter that tells readers whether it is new.
#[derive(Default)]
pub struct Latest(Mutex<(u64, Option<CFRetained<CVImageBuffer>>)>);

// SAFETY: CoreVideo buffers are reference counted atomically and a captured frame is never
// mutated, so sharing the handle between the capture queue and its readers is sound.
unsafe impl Send for Latest {}
unsafe impl Sync for Latest {}

impl Latest {
    pub fn store(&self, image: CFRetained<CVImageBuffer>) {
        let mut slot = self.0.lock().expect("frame slot poisoned");
        slot.0 += 1;
        slot.1 = Some(image);
    }

    pub fn get(&self) -> Option<CFRetained<CVImageBuffer>> {
        self.0.lock().expect("frame slot poisoned").1.clone()
    }

    fn newer_than(&self, seen: u64) -> Option<(u64, CFRetained<CVImageBuffer>)> {
        let slot = self.0.lock().expect("frame slot poisoned");
        match &slot.1 {
            Some(image) if slot.0 > seen => Some((slot.0, image.clone())),
            _ => None,
        }
    }
}

/// A preview frame: IOSurface-backed, `420f` with BT.601 colors, which is what gpui's surface
/// element draws, scaled down to at most 1920x1080 for the screen and 640x360 for the camera.
#[derive(Clone)]
pub struct PreviewFrame {
    pub buffer: CFRetained<CVPixelBuffer>,
    /// Counts up with every captured frame the preview converted, so a reader can tell whether
    /// the frame changed.
    pub sequence: u64,
}

// SAFETY: see `Latest`; a converted frame is not written after it is published.
unsafe impl Send for PreviewFrame {}
unsafe impl Sync for PreviewFrame {}

/// The latest screen and camera frames of a running [`Session`](crate::Session), for a live
/// preview. Cloning is cheap. The conversion thread stops when the last clone is dropped.
#[derive(Clone)]
pub struct Preview(Arc<Shared>);

struct Shared {
    raw: Sources,
    screen: Mutex<Option<PreviewFrame>>,
    camera: Mutex<Option<PreviewFrame>>,
}

/// The raw frame slots of a session.
#[derive(Clone, Default)]
pub struct Sources {
    pub screen: Arc<Latest>,
    pub camera: Arc<Latest>,
}

impl Preview {
    pub fn start(raw: Sources) -> Self {
        let shared = Arc::new(Shared {
            raw,
            screen: Mutex::new(None),
            camera: Mutex::new(None),
        });
        let weak = Arc::downgrade(&shared);
        std::thread::Builder::new()
            .name("preview converter".into())
            .spawn(move || run(weak))
            .expect("spawning the preview converter");
        Self(shared)
    }

    /// The newest screen frame, `None` until the first one arrived.
    pub fn screen(&self) -> Option<PreviewFrame> {
        self.0.screen.lock().expect("preview slot poisoned").clone()
    }

    /// The newest camera frame, `None` without a camera or before its first frame.
    pub fn camera(&self) -> Option<PreviewFrame> {
        self.0.camera.lock().expect("preview slot poisoned").clone()
    }
}

fn run(shared: Weak<Shared>) {
    let mut screen = Lane::new(SCREEN_MAX);
    let mut camera = Lane::new(CAMERA_MAX);
    let mut next = Instant::now();
    loop {
        next += REFRESH;
        std::thread::sleep(next.saturating_duration_since(Instant::now()));
        let Some(shared) = shared.upgrade() else {
            break;
        };
        screen.refresh(&shared.raw.screen, &shared.screen);
        camera.refresh(&shared.raw.camera, &shared.camera);
    }
}

/// Converts the frames of one source.
struct Lane {
    converter: Converter,
    seen: u64,
}

impl Lane {
    fn new(max: (usize, usize)) -> Self {
        Self {
            converter: Converter::new(max),
            seen: 0,
        }
    }

    fn refresh(&mut self, raw: &Latest, out: &Mutex<Option<PreviewFrame>>) {
        let Some((sequence, image)) = raw.newer_than(self.seen) else {
            return;
        };
        self.seen = sequence;
        if let Some(buffer) = self.converter.convert(&image) {
            *out.lock().expect("preview slot poisoned") = Some(PreviewFrame { buffer, sequence });
        }
    }
}

/// A VideoToolbox pixel transfer into pooled `420f` buffers.
struct Converter {
    session: CFRetained<VTPixelTransferSession>,
    max: (usize, usize),
    pool: Option<(usize, usize, CFRetained<CVPixelBufferPool>)>,
}

impl Converter {
    fn new(max: (usize, usize)) -> Self {
        let mut session = ptr::null_mut();
        // SAFETY: The out pointer is writable.
        let status = unsafe { VTPixelTransferSession::create(None, NonNull::from(&mut session)) };
        let session = NonNull::new(session)
            .filter(|_| status == 0)
            .expect("creating a pixel transfer session");
        // SAFETY: Create returns an owned session on success.
        let session = unsafe { CFRetained::<VTPixelTransferSession>::from_raw(session) };
        // BT.601 is what gpui's surface shader assumes; the transfer re-encodes the colors.
        // SAFETY: Both keys take the constant values CoreVideo documents for them.
        unsafe {
            VTSessionSetProperty(
                &session,
                kVTPixelTransferPropertyKey_DestinationYCbCrMatrix,
                Some(kCVImageBufferYCbCrMatrix_ITU_R_601_4),
            );
            VTSessionSetProperty(
                &session,
                kVTPixelTransferPropertyKey_RealTime,
                kCFBooleanTrue.map(|b| b as &CFType),
            );
        }
        Self {
            session,
            max,
            pool: None,
        }
    }

    fn convert(&mut self, image: &CVPixelBuffer) -> Option<CFRetained<CVPixelBuffer>> {
        let (width, height) = fit(
            CVPixelBufferGetWidth(image),
            CVPixelBufferGetHeight(image),
            self.max,
        );
        if !matches!(&self.pool, Some((w, h, _)) if (*w, *h) == (width, height)) {
            self.pool = Some((width, height, create_pool(width, height)));
        }
        let (_, _, pool) = self.pool.as_ref()?;
        let mut buffer = ptr::null_mut();
        // SAFETY: The pool is live and the out pointer is writable.
        let status = unsafe {
            CVPixelBufferPool::create_pixel_buffer(None, pool, NonNull::from(&mut buffer))
        };
        // SAFETY: Create returns an owned buffer on success.
        let buffer = unsafe { CFRetained::from_raw(NonNull::new(buffer)?) };
        // SAFETY: Both buffers are live pixel buffers.
        let status = (status == 0).then(|| unsafe { self.session.transfer_image(image, &buffer) });
        (status == Some(0)).then_some(buffer)
    }
}

/// The largest size inside `max` with the aspect of `width` x `height`, never above the source
/// and with even sides, which 4:2:0 needs.
fn fit(width: usize, height: usize, max: (usize, usize)) -> (usize, usize) {
    let scale = (max.0 as f64 / width as f64)
        .min(max.1 as f64 / height as f64)
        .min(1.0);
    let even = |side: usize| ((side as f64 * scale).round() as usize & !1).max(2);
    (even(width), even(height))
}

fn create_pool(width: usize, height: usize) -> CFRetained<CVPixelBufferPool> {
    let format = CFNumber::new_i32(kCVPixelFormatType_420YpCbCr8BiPlanarFullRange as i32);
    let width = CFNumber::new_isize(width as isize);
    let height = CFNumber::new_isize(height as isize);
    let no_surface_options = CFDictionary::<CFString, CFType>::empty();
    let metal: &CFType = CFBoolean::new(true);
    // SAFETY: The keys are constant CF strings and each value has the type CoreVideo documents.
    let attributes = unsafe {
        CFDictionary::<CFString, CFType>::from_slices(
            &[
                kCVPixelBufferPixelFormatTypeKey,
                kCVPixelBufferWidthKey,
                kCVPixelBufferHeightKey,
                kCVPixelBufferIOSurfacePropertiesKey,
                kCVPixelBufferMetalCompatibilityKey,
            ],
            &[&format, &width, &height, &no_surface_options, metal],
        )
    };
    let mut pool = ptr::null_mut();
    // SAFETY: The attributes dictionary is valid and the out pointer is writable.
    let status = unsafe {
        CVPixelBufferPool::create(
            None,
            None,
            Some(attributes.as_opaque()),
            NonNull::from(&mut pool),
        )
    };
    assert_eq!(status, 0, "creating the preview buffer pool");
    // SAFETY: Create returns an owned pool on success.
    unsafe { CFRetained::from_raw(NonNull::new(pool).expect("CoreVideo returns a pool")) }
}

#[cfg(test)]
mod tests {
    use super::fit;

    #[test]
    fn fit_keeps_the_aspect_inside_the_maximum() {
        assert_eq!(fit(3456, 2234, (1920, 1080)), (1670, 1080));
        assert_eq!(fit(1280, 720, (1920, 1080)), (1280, 720));
    }
}
