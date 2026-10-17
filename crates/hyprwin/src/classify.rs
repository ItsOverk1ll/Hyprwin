//! Which windows to manage, and which to float.
//!
//! Tiling things that shouldn't tile is the #1 way Windows tilers feel broken,
//! so this is deliberately conservative: anything unusual is ignored or floated.

use std::ffi::c_void;

use hyprwin_win32::{HWND, dwm, window};
use windows::Win32::Foundation::{CloseHandle, HANDLE, LPARAM};
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, GA_ROOT, GW_OWNER, GetAncestor, GetWindow, WS_CAPTION, WS_CHILD, WS_EX_APPWINDOW, WS_EX_DLGMODALFRAME,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_MAXIMIZEBOX, WS_POPUP, WS_THICKFRAME,
};
use windows::core::BOOL;

/// Per-window metadata used for rules, IPC and debugging.
#[derive(Clone, Debug, Default)]
pub struct Meta {
    pub class: String,
    pub winclass: String,
    pub title: String,
    pub exe: String,
    pub pid: u32,
    pub elevated: bool,
    pub style: u32,
    pub ex_style: u32,
    pub owner: isize,
}

pub fn read_meta(h: HWND) -> Meta {
    let winclass = window::class_name(h);
    let mut pid = window::pid(h);
    // UWP apps live inside ApplicationFrameHost; the real app is the CoreWindow child.
    if winclass == "ApplicationFrameWindow" {
        if let Some(p) = uwp_child_pid(h, pid) {
            pid = p;
        }
    }
    // One process query for both: `class` is derived from the image path.
    let exe = window::process_path(pid).unwrap_or_default();
    let class = window::class_from_path(&exe).unwrap_or_else(|| winclass.to_lowercase());
    let owner = unsafe { GetWindow(h, GW_OWNER) }.map(|o| o.0 as isize).unwrap_or(0);
    Meta {
        class,
        winclass,
        title: window::title(h),
        exe,
        pid,
        elevated: is_elevated(pid),
        style: window::style(h),
        ex_style: window::ex_style(h),
        owner,
    }
}

fn uwp_child_pid(h: HWND, host_pid: u32) -> Option<u32> {
    unsafe extern "system" fn cb(child: HWND, data: LPARAM) -> BOOL {
        let out = unsafe { &mut *(data.0 as *mut (u32, u32)) };
        let p = window::pid(child);
        if p != out.0 && window::class_name(child) == "Windows.UI.Core.CoreWindow" {
            out.1 = p;
            return false.into();
        }
        true.into()
    }
    let mut data = (host_pid, 0u32);
    unsafe {
        let _ = EnumChildWindows(Some(h), Some(cb), LPARAM(&mut data as *mut _ as isize));
    }
    (data.1 != 0).then_some(data.1)
}

fn token_elevated(process: HANDLE) -> Option<bool> {
    unsafe {
        let mut tok = HANDLE::default();
        OpenProcessToken(process, TOKEN_QUERY, &mut tok).ok()?;
        let mut e = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let r = GetTokenInformation(tok, TokenElevation, Some(&mut e as *mut _ as *mut c_void), std::mem::size_of::<TOKEN_ELEVATION>() as u32, &mut len);
        let _ = CloseHandle(tok);
        r.ok()?;
        Some(e.TokenIsElevated != 0)
    }
}

pub fn self_elevated() -> bool {
    token_elevated(unsafe { GetCurrentProcess() }).unwrap_or(false)
}

/// True when the process runs at a higher integrity than we can control (UIPI).
pub fn is_elevated(pid: u32) -> bool {
    static SELF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *SELF.get_or_init(self_elevated) {
        return false;
    }
    unsafe {
        let Ok(p) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false };
        // A token we cannot open belongs to a more privileged process.
        let r = token_elevated(p).unwrap_or(true);
        let _ = CloseHandle(p);
        r
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Ignore(&'static str),
    Manage { float: bool, reason: &'static str },
}

/// Win32 classes that are never app windows.
const IGNORED_CLASSES: &[&str] = &[
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "Progman",
    "WorkerW",
    "Windows.UI.Core.CoreWindow",
    "XamlExplorerHostIslandWindow",
    "TopLevelWindowForOverflowXamlIsland",
    "NotifyIconOverflowWindow",
    "#32768",
    "tooltips_class32",
    "ForegroundStaging",
    "MultitaskingViewFrame",
    "TaskListThumbnailWnd",
    "Windows.Internal.Shell.TabProxyWindow",
    "ApplicationManager_ImmersiveShellWindow",
    "EdgeUiInputTopWndClass",
    "NativeHWNDHost",
    "Shell_InputSwitchTopLevelWindow",
    "Shell_LightDismissOverlay",
    "SysShadow",
    "IME",
    "MSCTFIME UI",
    "Ghost",
    "PopupHost",
    "Xaml_WindowedPopupClass",
    "LockScreenControllerProxyWindow",
    "Windows.UI.Input.InputSite.WindowClass",
    "hyprwin.ghost",
    "hyprwin.main",
    "DummyDWMListenerWindow",
    "Chrome_RenderWidgetHostHWND",
    "CEF-OSC-WIDGET",
];

/// The cheap subset of [`classify`]'s `Ignore` checks, run before
/// [`read_meta`] (which opens the process). Windows sends create/show/name
/// events for every tooltip, menu and child control on the desktop, and
/// nearly all of them stop here.
pub fn quick_reject(h: HWND, own_pid: u32) -> Option<&'static str> {
    if !window::is_visible(h) {
        return Some("not visible");
    }
    if window::style(h) & WS_CHILD.0 != 0 || unsafe { GetAncestor(h, GA_ROOT) } != h {
        return Some("child window");
    }
    let ex = window::ex_style(h);
    if ex & WS_EX_TOOLWINDOW.0 != 0 && ex & WS_EX_APPWINDOW.0 == 0 {
        return Some("tool window");
    }
    if ex & WS_EX_NOACTIVATE.0 != 0 {
        return Some("no-activate window");
    }
    if window::pid(h) == own_pid {
        return Some("our own window");
    }
    if IGNORED_CLASSES.contains(&window::class_name(h).as_str()) {
        return Some("shell/system class");
    }
    None
}

pub fn classify(h: HWND, m: &Meta, own_pid: u32) -> Verdict {
    if !window::is_visible(h) {
        return Verdict::Ignore("not visible");
    }
    if m.pid == own_pid {
        return Verdict::Ignore("our own window");
    }
    if unsafe { GetAncestor(h, GA_ROOT) } != h || m.style & WS_CHILD.0 != 0 {
        return Verdict::Ignore("child window");
    }
    if IGNORED_CLASSES.contains(&m.winclass.as_str()) {
        return Verdict::Ignore("shell/system class");
    }
    if m.ex_style & WS_EX_TOOLWINDOW.0 != 0 && m.ex_style & WS_EX_APPWINDOW.0 == 0 {
        return Verdict::Ignore("tool window");
    }
    if m.ex_style & WS_EX_NOACTIVATE.0 != 0 {
        return Verdict::Ignore("no-activate window");
    }
    if dwm::cloaked_state(h) != 0 {
        return Verdict::Ignore("cloaked by the system");
    }
    if m.elevated {
        return Verdict::Ignore("elevated (run hyprwin elevated to manage admin windows)");
    }
    let Ok(r) = dwm::frame_bounds(h) else { return Verdict::Ignore("no frame") };
    if r.w < 40 || r.h < 40 {
        return Verdict::Ignore("too small");
    }
    let popup = m.style & WS_POPUP.0 != 0;
    let caption = m.style & WS_CAPTION.0 == WS_CAPTION.0;
    if m.title.is_empty() && (popup || !caption) {
        return Verdict::Ignore("untitled popup");
    }
    // From here on we manage it; decide tile vs float.
    // hyprwin's own installer is a dialog, whatever "tile everything" says.
    if m.title == "hyprwin Setup" && m.exe.to_lowercase().contains("hyprwin") {
        return Verdict::Manage { float: true, reason: "dialog" };
    }
    if m.owner != 0 {
        return Verdict::Manage { float: true, reason: "owned window (dialog/palette)" };
    }
    if m.winclass == "#32770" || m.ex_style & WS_EX_DLGMODALFRAME.0 != 0 {
        return Verdict::Manage { float: true, reason: "dialog" };
    }
    if m.ex_style & WS_EX_TOPMOST.0 != 0 {
        return Verdict::Manage { float: true, reason: "always-on-top" };
    }
    if m.style & WS_THICKFRAME.0 == 0 {
        return Verdict::Manage { float: true, reason: "not resizable" };
    }
    if popup && !caption && m.style & WS_MAXIMIZEBOX.0 == 0 {
        return Verdict::Manage { float: true, reason: "borderless popup" };
    }
    Verdict::Manage { float: false, reason: "normal app window" }
}
