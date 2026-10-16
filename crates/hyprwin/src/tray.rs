//! Notification-area (system tray) icon and its menu. Pure Win32: no extra
//! process, no extra memory to speak of.

use hyprwin_win32::{HWND, wide};
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NOTIFYICON_VERSION_4, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::PCWSTR;

pub const WM_APP_TRAY: u32 = WM_APP + 20;
const TRAY_ID: u32 = 1;

static TRAY_ICO: &[u8] = include_bytes!("../../../assets/icons/tray_icon.ico");

/// Picks the best image from an .ico file (PNG or BMP entries) for `size` px.
fn icon_from_ico(bytes: &[u8], size: i32) -> Option<HICON> {
    let count = u16::from_le_bytes([*bytes.get(4)?, *bytes.get(5)?]) as usize;
    let mut best: Option<(i32, usize, usize)> = None;
    for i in 0..count {
        let e = 6 + i * 16;
        let w = match *bytes.get(e)? {
            0 => 256,
            n => n as i32,
        };
        let len = u32::from_le_bytes(bytes.get(e + 8..e + 12)?.try_into().ok()?) as usize;
        let off = u32::from_le_bytes(bytes.get(e + 12..e + 16)?.try_into().ok()?) as usize;
        // Prefer the smallest image at least `size`, else the largest.
        let score = if w >= size { w - size } else { 10_000 + (size - w) };
        if best.is_none_or(|b| score < b.0) {
            best = Some((score, off, len));
        }
    }
    let (_, off, len) = best?;
    let data = bytes.get(off..off + len)?;
    unsafe { CreateIconFromResourceEx(data, true, 0x0003_0000, size, size, LR_DEFAULTCOLOR).ok() }
}

fn small_icon_size() -> i32 {
    unsafe {
        let dpi = windows::Win32::UI::HiDpi::GetDpiForSystem();
        windows::Win32::UI::HiDpi::GetSystemMetricsForDpi(SM_CXSMICON, dpi).max(16)
    }
}

fn data(main: HWND, paused: bool) -> NOTIFYICONDATAW {
    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: main,
        uID: TRAY_ID,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP,
        uCallbackMessage: WM_APP_TRAY,
        hIcon: icon_from_ico(TRAY_ICO, small_icon_size()).unwrap_or_default(),
        ..Default::default()
    };
    let tip = if paused { "hyprwin — paused" } else { "hyprwin — tiling" };
    for (i, c) in tip.encode_utf16().take(nid.szTip.len() - 1).enumerate() {
        nid.szTip[i] = c;
    }
    nid
}

pub fn add(main: HWND, paused: bool) {
    let mut nid = data(main, paused);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_ADD, &nid);
        nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        let _ = Shell_NotifyIconW(NIM_SETVERSION, &nid);
    }
}

pub fn update(main: HWND, paused: bool) {
    let nid = data(main, paused);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

/// Shows a notification from the tray icon (a Windows toast).
pub fn notify(main: HWND, title: &str, text: &str) {
    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: main,
        uID: TRAY_ID,
        uFlags: windows::Win32::UI::Shell::NIF_INFO,
        dwInfoFlags: windows::Win32::UI::Shell::NIIF_USER | windows::Win32::UI::Shell::NIIF_LARGE_ICON,
        ..Default::default()
    };
    for (i, c) in title.encode_utf16().take(nid.szInfoTitle.len() - 1).enumerate() {
        nid.szInfoTitle[i] = c;
    }
    for (i, c) in text.encode_utf16().take(nid.szInfo.len() - 1).enumerate() {
        nid.szInfo[i] = c;
    }
    nid.hBalloonIcon = icon_from_ico(TRAY_ICO, 32).unwrap_or_default();
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

/// On the very first start, tells the user where hyprwin lives and how to
/// get out. Remembered with a marker file in the state folder.
pub fn welcome_once(main: HWND) {
    let marker = hyprwin_win32::ledger::state_dir().join("welcomed");
    // Live tests run with a fresh state folder and a manage filter: no toasts.
    if marker.exists() || std::env::var_os("HYPRWIN_MANAGE_ONLY").is_some() {
        return;
    }
    let _ = std::fs::write(&marker, "");
    notify(
        main,
        "hyprwin is tiling your windows",
        "Right-click the hyprwin tray icon for Settings, Themes and Pause. Super+Q opens a terminal. Emergency exit: Ctrl+Alt+Shift+Win+Esc.",
    );
}

pub fn remove(main: HWND) {
    let nid = NOTIFYICONDATAW { cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32, hWnd: main, uID: TRAY_ID, ..Default::default() };
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum MenuCmd {
    TogglePause,
    Settings,
    Themes,
    ManageGroups,
    SaveGroup,
    OpenGroup(String),
    OpenConfig,
    OpenLog,
    Exit,
}

const ID_PAUSE: usize = 1;
const ID_SETTINGS: usize = 2;
const ID_THEMES: usize = 3;
const ID_GROUPS_MANAGE: usize = 4;
const ID_GROUPS_SAVE: usize = 5;
const ID_CONFIG: usize = 6;
const ID_LOG: usize = 7;
const ID_EXIT: usize = 8;
const ID_GROUP_BASE: usize = 100;

fn append(menu: HMENU, flags: MENU_ITEM_FLAGS, id: usize, text: &str) {
    let w = wide(text);
    unsafe {
        let _ = AppendMenuW(menu, flags, id, PCWSTR(w.as_ptr()));
    }
}

/// Shows the tray menu at the cursor and returns what was picked.
pub fn show_menu(main: HWND, paused: bool, groups: &[(String, usize)]) -> Option<MenuCmd> {
    unsafe {
        let menu = CreatePopupMenu().ok()?;
        append(menu, MF_STRING | MF_DISABLED, 0, "hyprwin");
        append(menu, MF_SEPARATOR, 0, "");
        append(menu, MF_STRING, ID_PAUSE, if paused { "Resume tiling" } else { "Pause tiling" });
        let sub = CreatePopupMenu().ok()?;
        if groups.is_empty() {
            append(sub, MF_STRING | MF_GRAYED, 0, "(no saved groups yet)");
        }
        for (i, (name, n)) in groups.iter().enumerate().take(50) {
            append(sub, MF_STRING, ID_GROUP_BASE + i, &format!("Open \"{name}\"  ({n} apps)"));
        }
        append(sub, MF_SEPARATOR, 0, "");
        append(sub, MF_STRING | if paused { MF_GRAYED } else { MF_ENABLED }, ID_GROUPS_SAVE, "Save this workspace as a group…");
        append(sub, MF_STRING, ID_GROUPS_MANAGE, "Manage groups…");
        let _ = AppendMenuW(menu, MF_POPUP, sub.0 as usize, PCWSTR(wide("App Groups").as_ptr()));
        append(menu, MF_STRING, ID_THEMES, "Themes…");
        append(menu, MF_STRING, ID_SETTINGS, "Settings…");
        append(menu, MF_SEPARATOR, 0, "");
        append(menu, MF_STRING, ID_CONFIG, "Edit config file");
        append(menu, MF_STRING, ID_LOG, "Open log");
        append(menu, MF_SEPARATOR, 0, "");
        append(menu, MF_STRING, ID_EXIT, "Exit hyprwin");
        let _ = SetMenuDefaultItem(menu, ID_SETTINGS as u32, 0);

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        // Required so the menu closes when the user clicks elsewhere.
        let _ = SetForegroundWindow(main);
        let picked = TrackPopupMenuEx(menu, (TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY).0, pt.x, pt.y, main, None).0 as usize;
        let _ = PostMessageW(Some(main), WM_NULL, Default::default(), Default::default());
        let _ = DestroyMenu(menu);
        Some(match picked {
            ID_PAUSE => MenuCmd::TogglePause,
            ID_SETTINGS => MenuCmd::Settings,
            ID_THEMES => MenuCmd::Themes,
            ID_GROUPS_MANAGE => MenuCmd::ManageGroups,
            ID_GROUPS_SAVE => MenuCmd::SaveGroup,
            ID_CONFIG => MenuCmd::OpenConfig,
            ID_LOG => MenuCmd::OpenLog,
            ID_EXIT => MenuCmd::Exit,
            n if n >= ID_GROUP_BASE && n - ID_GROUP_BASE < groups.len() => MenuCmd::OpenGroup(groups[n - ID_GROUP_BASE].0.clone()),
            _ => return None,
        })
    }
}
