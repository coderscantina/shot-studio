//! AppKit and Carbon calls gpui has no API for: display names, a borderless window that covers
//! one display, the mirror flip, and global hotkeys.

use std::ffi::{CStr, c_char, c_void};

use gpui::Window;
use objc2::runtime::{AnyObject, Bool};
use objc2::{class, msg_send};
use objc2_core_foundation::{CGAffineTransform, CGPoint, CGRect};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// Connected displays as (CoreGraphics display id, name), like "Elgato Prom.". Main thread only.
pub fn display_names() -> Vec<(u32, String)> {
    let mut out = Vec::new();
    // SAFETY: NSScreen, NSDictionary and NSString calls with their documented signatures, on
    // the main thread where gpui calls this.
    unsafe {
        let screens: *mut AnyObject = msg_send![class!(NSScreen), screens];
        let count: usize = msg_send![screens, count];
        let key: *mut AnyObject =
            msg_send![class!(NSString), stringWithUTF8String: c"NSScreenNumber".as_ptr()];
        for i in 0..count {
            let screen: *mut AnyObject = msg_send![screens, objectAtIndex: i];
            let description: *mut AnyObject = msg_send![screen, deviceDescription];
            let number: *mut AnyObject = msg_send![description, objectForKey: key];
            if number.is_null() {
                continue;
            }
            let id: u32 = msg_send![number, unsignedIntValue];
            let name: *mut AnyObject = msg_send![screen, localizedName];
            let utf8: *const c_char = msg_send![name, UTF8String];
            if !utf8.is_null() {
                out.push((id, CStr::from_ptr(utf8).to_string_lossy().into_owned()));
            }
        }
    }
    out
}

fn native_view(window: &Window) -> Option<*mut AnyObject> {
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::AppKit(handle) => Some(handle.ns_view.as_ptr().cast()),
        _ => None,
    }
}

/// Makes the window borderless and sizes it to its screen's full frame, over the menu bar,
/// without a fullscreen Space. It stays visible when the app is in the background.
pub fn cover_screen(window: &Window) {
    let Some(view) = native_view(window) else {
        return;
    };
    // SAFETY: the view is gpui's live NSView, and these are plain NSWindow setters.
    unsafe {
        let native: *mut AnyObject = msg_send![view, window];
        let screen: *mut AnyObject = msg_send![native, screen];
        if native.is_null() || screen.is_null() {
            return;
        }
        let frame: CGRect = msg_send![screen, frame];
        let () = msg_send![native, setStyleMask: 0usize];
        let () = msg_send![native, setFrame: frame, display: Bool::YES];
        let () = msg_send![native, setHidesOnDeactivate: Bool::NO];
        let () = msg_send![native, setHasShadow: Bool::NO];
        // NSWindowCollectionBehaviorStationary | IgnoresCycle | FullScreenNone: not part of
        // Exposé, window cycling or a fullscreen Space.
        let () = msg_send![native, setCollectionBehavior: (1usize << 4) | (1 << 6) | (1 << 9)];
    }
}

/// Flips the window's content horizontally in the compositor when `on`, for a beam splitter
/// that shows the screen mirrored.
pub fn set_mirror(window: &Window, on: bool) {
    let Some(view) = native_view(window) else {
        return;
    };
    // SAFETY: gpui's view is backed by its CAMetalLayer, which takes an affine transform.
    unsafe {
        let layer: *mut AnyObject = msg_send![view, layer];
        if layer.is_null() {
            return;
        }
        // AppKit anchors a view's layer at its corner, so the flip is moved back into place.
        let anchor: CGPoint = msg_send![layer, anchorPoint];
        let bounds: CGRect = msg_send![layer, bounds];
        let transform = CGAffineTransform {
            a: if on { -1.0 } else { 1.0 },
            b: 0.0,
            c: 0.0,
            d: 1.0,
            tx: if on {
                bounds.size.width * (1.0 - 2.0 * anchor.x)
            } else {
                0.0
            },
            ty: 0.0,
        };
        let () = msg_send![layer, setAffineTransform: transform];
    }
}

/// Carbon modifier masks for [`Hotkeys::register`].
pub const CONTROL: u32 = 1 << 12;
pub const OPTION: u32 = 1 << 11;

#[repr(C)]
struct EventTypeSpec {
    class: u32,
    kind: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct EventHotKeyId {
    signature: u32,
    id: u32,
}

type Handler = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> i32;

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn GetApplicationEventTarget() -> *mut c_void;
    fn InstallEventHandler(
        target: *mut c_void,
        handler: Handler,
        count: u32,
        types: *const EventTypeSpec,
        user_data: *mut c_void,
        out: *mut *mut c_void,
    ) -> i32;
    fn RemoveEventHandler(handler: *mut c_void) -> i32;
    fn RegisterEventHotKey(
        key_code: u32,
        modifiers: u32,
        id: EventHotKeyId,
        target: *mut c_void,
        options: u32,
        out: *mut *mut c_void,
    ) -> i32;
    fn UnregisterEventHotKey(hotkey: *mut c_void) -> i32;
    fn GetEventParameter(
        event: *mut c_void,
        name: u32,
        kind: u32,
        actual_kind: *mut u32,
        size: usize,
        actual_size: *mut usize,
        data: *mut c_void,
    ) -> i32;
}

const fn four(code: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*code)
}

const KEYBOARD: u32 = four(b"keyb");
const HOT_KEY_PRESSED: u32 = 5;
const DIRECT_OBJECT: u32 = four(b"----");
const HOT_KEY_ID: u32 = four(b"hkid");
const SIGNATURE: u32 = four(b"shot");

type Callback = Box<dyn Fn(usize)>;

/// System-wide hotkeys through Carbon's `RegisterEventHotKey`. The system hands only the exact
/// key and modifier combination to this app, consumes it, and leaves every other key alone. It
/// needs no permission. Dropping it unregisters every key.
pub struct Hotkeys {
    handler: *mut c_void,
    keys: Vec<*mut c_void>,
    _callback: Box<Callback>,
}

impl Hotkeys {
    /// Registers each (virtual key code, Carbon modifiers) and calls `on_press` on the main
    /// thread with the index of the key pressed. A key another app holds is skipped.
    pub fn register(keys: &[(u16, u32)], on_press: impl Fn(usize) + 'static) -> Self {
        let callback: Box<Callback> = Box::new(Box::new(on_press));
        let mut handler = std::ptr::null_mut();
        let mut registered = Vec::new();
        // SAFETY: the callback box outlives the handler, which `drop` removes first.
        unsafe {
            let target = GetApplicationEventTarget();
            let spec = EventTypeSpec {
                class: KEYBOARD,
                kind: HOT_KEY_PRESSED,
            };
            InstallEventHandler(
                target,
                pressed,
                1,
                &spec,
                (&*callback as *const Callback).cast_mut().cast(),
                &mut handler,
            );
            for (index, &(code, modifiers)) in keys.iter().enumerate() {
                let id = EventHotKeyId {
                    signature: SIGNATURE,
                    id: index as u32,
                };
                let mut key = std::ptr::null_mut();
                if RegisterEventHotKey(u32::from(code), modifiers, id, target, 0, &mut key) == 0 {
                    registered.push(key);
                }
            }
        }
        Self {
            handler,
            keys: registered,
            _callback: callback,
        }
    }
}

unsafe extern "C" fn pressed(_: *mut c_void, event: *mut c_void, user_data: *mut c_void) -> i32 {
    let mut id = EventHotKeyId {
        signature: 0,
        id: 0,
    };
    // SAFETY: a hot key event carries its id as the direct object; user_data is the callback.
    let status = unsafe {
        GetEventParameter(
            event,
            DIRECT_OBJECT,
            HOT_KEY_ID,
            std::ptr::null_mut(),
            size_of::<EventHotKeyId>(),
            std::ptr::null_mut(),
            (&raw mut id).cast(),
        )
    };
    if status == 0 && id.signature == SIGNATURE {
        let callback = unsafe { &*user_data.cast::<Callback>() };
        callback(id.id as usize);
    }
    0
}

impl Drop for Hotkeys {
    fn drop(&mut self) {
        // SAFETY: each pointer came from a successful registration.
        unsafe {
            for key in self.keys.drain(..) {
                UnregisterEventHotKey(key);
            }
            if !self.handler.is_null() {
                RemoveEventHandler(self.handler);
            }
        }
    }
}
