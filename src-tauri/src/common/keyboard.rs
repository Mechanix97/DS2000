//! Whether Shift is held on the computer's keyboard, read at the moment a device key arrives.
//!
//! The device only reports presses, never releases, so a modifier on the device itself cannot be
//! told apart from two separate presses. The keyboard's Shift needs no protocol change.
//!
//! Each platform asks its own API for the global keyboard state:
//! - Windows: `GetAsyncKeyState`.
//! - macOS: `CGEventSourceFlagsState`.
//! - Linux on X11: `XQueryKeymap`, through libX11 loaded at run time.
//!
//! Wayland does not expose the global keyboard state to applications, and XWayland only sees
//! keys while one of its own windows has focus, so there the answer is "unknown".

/// Whether Shift is held now. `None` when this platform or session cannot tell.
pub fn shift_held() -> Option<bool> {
    platform::shift_held()
}

/// Whether [`shift_held`] can answer here, for the settings to say when it cannot.
pub fn shift_detectable() -> bool {
    shift_held().is_some()
}

#[cfg(windows)]
mod platform {
    const VK_SHIFT: i32 = 0x10;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetAsyncKeyState(virtual_key: i32) -> i16;
    }

    pub fn shift_held() -> Option<bool> {
        // The most significant bit is set while the key is down.
        // SAFETY: takes a plain integer and reads global state; no pointers involved.
        let state = unsafe { GetAsyncKeyState(VK_SHIFT) };
        Some(state < 0)
    }
}

#[cfg(target_os = "macos")]
mod platform {
    /// `kCGEventSourceStateCombinedSessionState`: every keyboard in the login session.
    const COMBINED_SESSION_STATE: i32 = 0;
    /// `kCGEventFlagMaskShift`.
    const FLAG_MASK_SHIFT: u64 = 0x0002_0000;

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventSourceFlagsState(state_id: i32) -> u64;
    }

    pub fn shift_held() -> Option<bool> {
        // SAFETY: takes a plain integer and returns the modifier flags; no pointers involved.
        let flags = unsafe { CGEventSourceFlagsState(COMBINED_SESSION_STATE) };
        Some(flags & FLAG_MASK_SHIFT != 0)
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use std::os::raw::c_char;
    use x11_dl::keysym::{XK_Shift_L, XK_Shift_R};
    use x11_dl::xlib::{KeySym, Xlib};

    fn on_wayland() -> bool {
        std::env::var_os("WAYLAND_DISPLAY").is_some()
            || std::env::var("XDG_SESSION_TYPE").is_ok_and(|session| session == "wayland")
    }

    pub fn shift_held() -> Option<bool> {
        if on_wayland() {
            return None;
        }
        // Loaded per call rather than kept: a key press is rare, and a display connection held
        // open would have to be shared across threads, which Xlib is not built for.
        let xlib = Xlib::open().ok()?;
        // SAFETY: a null name opens the display named by $DISPLAY; checked for null below.
        let display = unsafe { (xlib.XOpenDisplay)(std::ptr::null()) };
        if display.is_null() {
            return None;
        }

        // One bit per keycode, 256 of them.
        let mut keys: [c_char; 32] = [0; 32];
        // SAFETY: `display` is open and `keys` is the 32-byte buffer XQueryKeymap fills.
        unsafe { (xlib.XQueryKeymap)(display, keys.as_mut_ptr()) };
        let held = [XK_Shift_L, XK_Shift_R].into_iter().any(|keysym| {
            // SAFETY: `display` is open.
            let code = unsafe { (xlib.XKeysymToKeycode)(display, KeySym::from(keysym)) } as usize;
            code != 0 && (keys[code / 8] as u8) & (1 << (code % 8)) != 0
        });
        // SAFETY: opened above and not used after this.
        unsafe { (xlib.XCloseDisplay)(display) };
        Some(held)
    }
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
mod platform {
    pub fn shift_held() -> Option<bool> {
        None
    }
}
