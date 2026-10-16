//! Small shell/OS helpers used by the daemon.

use std::os::windows::process::CommandExt;

use hyprwin_win32::HWND;
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows::Win32::System::Threading::GetCurrentProcess;
use windows::Win32::UI::Input::KeyboardAndMouse::{INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY};
use windows::Win32::UI::Shell::{ITaskbarList2, TaskbarList};
use windows::Win32::UI::WindowsAndMessaging::{
    GUI_INMENUMODE, GUI_INMOVESIZE, GUI_POPUPMENUMODE, GUI_SYSTEMMENUMODE, GUITHREADINFO, GetForegroundWindow, GetGUIThreadInfo,
    GetWindowThreadProcessId,
};

/// Runs a command like Hyprland's `exec` (through the shell, detached, no console).
pub fn exec(cmd: &str, env: &[(String, String)]) {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    let home = std::env::var_os("USERPROFILE").unwrap_or_default();
    let mut c = std::process::Command::new("cmd");
    c.args(["/D", "/C"]).raw_arg(cmd).current_dir(home).creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
    for (k, v) in env {
        c.env(k, v);
    }
    match c.spawn() {
        Ok(_) => crate::info!("exec: {cmd}"),
        Err(e) => crate::warn!("exec '{cmd}' failed: {e}"),
    }
}

/// Injects an unassigned key so this process counts as having received the
/// last input, which Windows requires before `SetForegroundWindow` succeeds.
pub fn inject_noop() {
    let ev = |up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0xE8),
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                time: 0,
                dwExtraInfo: crate::input::MARK,
            },
        },
    };
    unsafe {
        SendInput(&[ev(false), ev(true)], std::mem::size_of::<INPUT>() as i32);
    }
}

/// True while a menu, popup menu, or move/size loop is active in the foreground app.
pub fn menu_active() -> bool {
    unsafe {
        let fg = GetForegroundWindow();
        let tid = GetWindowThreadProcessId(fg, None);
        let mut gi = GUITHREADINFO { cbSize: std::mem::size_of::<GUITHREADINFO>() as u32, ..Default::default() };
        if GetGUIThreadInfo(tid, &mut gi).is_err() {
            return false;
        }
        gi.flags.0 & (GUI_INMENUMODE.0 | GUI_POPUPMENUMODE.0 | GUI_SYSTEMMENUMODE.0 | GUI_INMOVESIZE.0) != 0
    }
}

/// Tells the taskbar a window is fullscreen (so it drops below it).
pub fn mark_fullscreen(h: HWND, on: bool) {
    unsafe {
        hyprwin_win32::com_init();
        if let Ok(tb) = CoCreateInstance::<_, ITaskbarList2>(&TaskbarList, None, CLSCTX_ALL) {
            let _ = tb.HrInit();
            let _ = tb.MarkFullscreenWindow(h, on);
        }
    }
}

pub fn working_set_mb() -> f64 {
    let mut pmc = PROCESS_MEMORY_COUNTERS { cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32, ..Default::default() };
    unsafe {
        let _ = GetProcessMemoryInfo(GetCurrentProcess(), &mut pmc, pmc.cb);
    }
    pmc.WorkingSetSize as f64 / (1024.0 * 1024.0)
}
