//! Cursor and key events from a listen-only CGEventTap, written to `events.jsonl` as they happen.

use std::ffi::c_void;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;

use objc2_core_foundation::{CFMachPort, CFRetained, CFRunLoop, kCFRunLoopDefaultMode};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventTapProxy, CGEventType, CGPreflightListenEventAccess, CGRequestListenEventAccess,
};
use objc2_core_media::CMClock;

use crate::clock::{MediaClock, nanos};
use crate::events::{Button, DisplayGeometry, Event, EventKind, key_name, modifiers};
use crate::{CaptureError, Permission};

pub fn check_permission() -> Result<(), CaptureError> {
    if CGPreflightListenEventAccess() {
        return Ok(());
    }
    CGRequestListenEventAccess();
    Err(CaptureError::Permission(Permission::InputMonitoring))
}

/// A running event tap on its own run loop thread.
pub struct EventTap {
    stop: Arc<AtomicBool>,
    count: Arc<AtomicU64>,
    thread: JoinHandle<Result<(), String>>,
}

struct State {
    file: File,
    clock: MediaClock,
    host: CFRetained<CMClock>,
    geometry: DisplayGeometry,
    count: Arc<AtomicU64>,
    port: Option<CFRetained<CFMachPort>>,
    error: Option<String>,
}

impl EventTap {
    /// Starts logging to `path`. `origin_nanos` is media time zero on the host clock.
    pub fn start(
        path: &Path,
        origin_nanos: i64,
        geometry: DisplayGeometry,
    ) -> Result<Self, CaptureError> {
        let file = File::create(path).map_err(|e| CaptureError::Write {
            file: "events.jsonl",
            message: e.to_string(),
        })?;
        let stop = Arc::new(AtomicBool::new(false));
        let count = Arc::new(AtomicU64::new(0));
        let (ready, started) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("events.jsonl".into())
            .spawn({
                let stop = stop.clone();
                let count = count.clone();
                move || {
                    let state = State {
                        file,
                        clock: MediaClock::new(origin_nanos),
                        host: unsafe { CMClock::host_time_clock() },
                        geometry,
                        count,
                        port: None,
                        error: None,
                    };
                    run(state, &stop, ready)
                }
            })
            .expect("spawning the event thread");
        match started.recv() {
            Ok(true) => Ok(Self {
                stop,
                count,
                thread,
            }),
            _ => {
                let _ = thread.join();
                Err(CaptureError::Permission(Permission::InputMonitoring))
            }
        }
    }

    /// Stops the tap and returns the number of events written.
    pub fn stop(self) -> Result<u64, CaptureError> {
        self.stop.store(true, Ordering::Relaxed);
        self.thread
            .join()
            .expect("event thread panicked")
            .map_err(|message| CaptureError::Write {
                file: "events.jsonl",
                message,
            })?;
        Ok(self.count.load(Ordering::Relaxed))
    }
}

fn run(state: State, stop: &AtomicBool, ready: mpsc::Sender<bool>) -> Result<(), String> {
    let mask = [
        CGEventType::MouseMoved,
        CGEventType::LeftMouseDown,
        CGEventType::LeftMouseUp,
        CGEventType::RightMouseDown,
        CGEventType::RightMouseUp,
        CGEventType::LeftMouseDragged,
        CGEventType::RightMouseDragged,
        CGEventType::OtherMouseDown,
        CGEventType::OtherMouseUp,
        CGEventType::OtherMouseDragged,
        CGEventType::ScrollWheel,
        CGEventType::KeyDown,
    ]
    .iter()
    .fold(0u64, |mask, t| mask | 1 << t.0);
    let state = Box::into_raw(Box::new(state));
    // SAFETY: the callback only reads `state` on this thread, and the box outlives the tap.
    let port = unsafe {
        CGEvent::tap_create(
            CGEventTapLocation::SessionEventTap,
            CGEventTapPlacement::HeadInsertEventTap,
            CGEventTapOptions::ListenOnly,
            mask,
            Some(callback),
            state.cast(),
        )
    };
    let Some(port) = port else {
        drop(unsafe { Box::from_raw(state) });
        let _ = ready.send(false);
        return Ok(());
    };
    let source = CFMachPort::new_run_loop_source(None, Some(&port), 0);
    let run_loop = CFRunLoop::current().expect("a thread has a run loop");
    let mode = unsafe { kCFRunLoopDefaultMode };
    run_loop.add_source(source.as_deref(), mode);
    unsafe { (*state).port = Some(port.clone()) };
    let _ = ready.send(true);
    // Short runs notice the stop flag without a cross-thread wake up.
    while !stop.load(Ordering::Relaxed) {
        CFRunLoop::run_in_mode(mode, 0.1, false);
    }
    CGEvent::tap_enable(&port, false);
    run_loop.remove_source(source.as_deref(), mode);
    let mut state = unsafe { Box::from_raw(state) };
    state.port = None;
    match state.error.take() {
        Some(message) => Err(message),
        None => state.file.sync_all().map_err(|e| e.to_string()),
    }
}

unsafe extern "C-unwind" fn callback(
    _proxy: CGEventTapProxy,
    kind: CGEventType,
    event: NonNull<CGEvent>,
    user_info: *mut c_void,
) -> *mut CGEvent {
    // SAFETY: user_info is the State box `run` owns until the tap is gone.
    let state = unsafe { &mut *user_info.cast::<State>() };
    let event_ref = unsafe { event.as_ref() };
    if matches!(
        kind,
        CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput
    ) {
        if let Some(port) = &state.port {
            CGEvent::tap_enable(port, true);
        }
        return event.as_ptr();
    }
    let now = unsafe { state.host.time() };
    if let (Some(t), Some(kind)) = (
        state.clock.media_time(nanos(now.value, now.timescale)),
        event_kind(kind, event_ref, &state.geometry),
    ) && state.error.is_none()
    {
        let mut line = Event::new(t, kind).to_line();
        line.push('\n');
        match state.file.write_all(line.as_bytes()) {
            Ok(()) => {
                state.count.fetch_add(1, Ordering::Relaxed);
            }
            Err(e) => state.error = Some(e.to_string()),
        }
    }
    event.as_ptr()
}

fn event_kind(kind: CGEventType, event: &CGEvent, geometry: &DisplayGeometry) -> Option<EventKind> {
    let field = |f: CGEventField| CGEvent::integer_value_field(Some(event), f);
    let at = CGEvent::location(Some(event));
    let (x, y) = geometry.to_pixels(at.x, at.y);
    Some(match kind {
        CGEventType::MouseMoved
        | CGEventType::LeftMouseDragged
        | CGEventType::RightMouseDragged
        | CGEventType::OtherMouseDragged => EventKind::Move { x, y },
        CGEventType::LeftMouseDown => EventKind::Down {
            x,
            y,
            button: Button::Left,
        },
        CGEventType::RightMouseDown => EventKind::Down {
            x,
            y,
            button: Button::Right,
        },
        CGEventType::OtherMouseDown => EventKind::Down {
            x,
            y,
            button: Button::Other,
        },
        CGEventType::LeftMouseUp => EventKind::Up {
            x,
            y,
            button: Button::Left,
        },
        CGEventType::RightMouseUp => EventKind::Up {
            x,
            y,
            button: Button::Right,
        },
        CGEventType::OtherMouseUp => EventKind::Up {
            x,
            y,
            button: Button::Other,
        },
        CGEventType::ScrollWheel => {
            let dy = field(CGEventField::ScrollWheelEventPointDeltaAxis1);
            let dx = field(CGEventField::ScrollWheelEventPointDeltaAxis2);
            if dx == 0 && dy == 0 {
                return None;
            }
            EventKind::Scroll {
                x,
                y,
                dx: geometry.scale_distance(dx as f64),
                dy: geometry.scale_distance(dy as f64),
            }
        }
        CGEventType::KeyDown => {
            if field(CGEventField::KeyboardEventAutorepeat) != 0 {
                return None;
            }
            let keycode = field(CGEventField::KeyboardEventKeycode) as u16;
            let mut chars = [0u16; 4];
            let mut len = 0;
            unsafe {
                CGEvent::keyboard_get_unicode_string(
                    Some(event),
                    chars.len() as _,
                    &mut len,
                    chars.as_mut_ptr(),
                )
            };
            let typed = String::from_utf16_lossy(&chars[..(len as usize).min(chars.len())]);
            EventKind::Key {
                key: key_name(keycode, &typed)?,
                mods: modifiers(CGEvent::flags(Some(event)).0, keycode),
            }
        }
        _ => return None,
    })
}
