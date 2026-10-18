//! Every Windows API call in hyprwin goes through this crate.
//!
//! All operations on foreign windows are fallible and non-fatal: "access denied"
//! (elevated windows), "invalid handle" (window just died) and hung apps are
//! normal, and callers are expected to log and carry on.

pub mod clock;
pub mod cloak;
pub mod crash;
pub mod dpi;
pub mod dwm;
pub mod ghost;
pub mod ledger;
pub mod monitor;
pub mod window;

pub use hyprwin_anim::Rect;
pub use windows::Win32::Foundation::HWND;

pub type Result<T> = windows::core::Result<T>;

/// Converts a Rust string to a NUL-terminated UTF-16 buffer.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Initialises COM (STA) for the current thread. Safe to call repeatedly.
pub fn com_init() {
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    // S_FALSE (already initialised) and RPC_E_CHANGED_MODE are both fine for us.
    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
}

/// Pumps all pending messages for the current thread without blocking.
pub fn pump_messages() {
    use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage};
    let mut msg = MSG::default();
    unsafe {
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
