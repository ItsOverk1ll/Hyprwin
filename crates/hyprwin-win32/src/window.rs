use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GWL_EXSTYLE, GWL_STYLE, GetClassNameW, GetWindowLongPtrW, GetWindowRect, GetWindowTextW,
    GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, SET_WINDOW_POS_FLAGS, SWP_ASYNCWINDOWPOS,
    SWP_NOACTIVATE, SWP_NOCOPYBITS, SWP_NOOWNERZORDER, SWP_NOZORDER, SetWindowPos,
};
use windows::core::{BOOL, PWSTR};

use crate::Rect;
use crate::dwm::rect_from;

pub fn window_rect(hwnd: HWND) -> windows::core::Result<Rect> {
    let mut r = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut r)? };
    Ok(rect_from(r))
}

pub fn is_alive(hwnd: HWND) -> bool {
    unsafe { IsWindow(Some(hwnd)).as_bool() }
}

pub fn is_visible(hwnd: HWND) -> bool {
    unsafe { IsWindowVisible(hwnd).as_bool() }
}

pub fn is_minimized(hwnd: HWND) -> bool {
    unsafe { IsIconic(hwnd).as_bool() }
}

pub fn title(hwnd: HWND) -> String {
    let mut buf = [0u16; 512];
    let n = unsafe { GetWindowTextW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

pub fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

pub fn style(hwnd: HWND) -> u32 {
    unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) as u32 }
}

pub fn ex_style(hwnd: HWND) -> u32 {
    unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 }
}

pub fn pid(hwnd: HWND) -> u32 {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid
}

/// Full image path of a process, or `None` (e.g. protected processes).
pub fn process_path(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let r = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(h);
        r.ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// Hyprland-style `class`: executable file name without `.exe`, lowercased.
pub fn process_class(pid: u32) -> Option<String> {
    class_from_path(&process_path(pid)?)
}

/// `class` from an image path (see [`process_class`]).
pub fn class_from_path(path: &str) -> Option<String> {
    let file = path.rsplit(['\\', '/']).next().filter(|f| !f.is_empty())?;
    let lower = file.to_lowercase();
    Some(lower.strip_suffix(".exe").map(str::to_string).unwrap_or(lower))
}

/// All top-level windows in z-order (topmost first).
pub fn top_level_windows() -> Vec<HWND> {
    unsafe extern "system" fn cb(hwnd: HWND, data: LPARAM) -> BOOL {
        let v = unsafe { &mut *(data.0 as *mut Vec<HWND>) };
        v.push(hwnd);
        true.into()
    }
    let mut v: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut v as *mut _ as isize));
    }
    v
}

/// Moves/resizes a window to an *outer* rect. Asynchronous: a hung target app
/// can never block the caller.
pub fn set_outer_rect(hwnd: HWND, r: Rect) -> windows::core::Result<()> {
    let flags: SET_WINDOW_POS_FLAGS = SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_ASYNCWINDOWPOS | SWP_NOCOPYBITS;
    unsafe { SetWindowPos(hwnd, None, r.x, r.y, r.w, r.h, flags) }
}

/// Synchronous variant (used when we must read back the result immediately).
pub fn set_outer_rect_sync(hwnd: HWND, r: Rect) -> windows::core::Result<()> {
    let flags: SET_WINDOW_POS_FLAGS = SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_NOCOPYBITS;
    unsafe { SetWindowPos(hwnd, None, r.x, r.y, r.w, r.h, flags) }
}

/// Places a window so its *visible* frame lands exactly on `visible`,
/// correcting for the invisible resize borders.
pub fn set_visible_rect(hwnd: HWND, visible: Rect) -> windows::core::Result<()> {
    let insets = crate::dwm::invisible_insets(hwnd).unwrap_or_default();
    set_outer_rect(hwnd, insets.outer(visible))
}
