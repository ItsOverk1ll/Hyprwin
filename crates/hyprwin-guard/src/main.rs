//! hyprwin-guard: the safety net.
//!
//! - Owns the hard-coded emergency combo **Ctrl+Alt+Shift+Win+Esc**
//!   (`RegisterHotKey`, so it works even if the daemon's hooks are hung).
//!   Pressing it restores every window and the taskbar and kills the daemon.
//! - Waits on the daemon process. If it dies without cleaning up, replays the
//!   ledger so no window is ever left hidden.

#![windows_subsystem = "windows"]

use hyprwin_win32::ledger::Ledger;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{
    CreateMutexW, OpenProcess, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, TerminateProcess,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey, VK_ESCAPE};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MSG, MsgWaitForMultipleObjects, PM_REMOVE, PeekMessageW, QS_ALLINPUT, WM_HOTKEY,
};
use windows::core::PCWSTR;

fn log(msg: &str) {
    use std::io::Write;
    let dir = hyprwin_win32::ledger::state_dir();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("hyprwin.log")) {
        let _ = writeln!(f, "guard: {msg}");
    }
}

fn restore(reason: &str) {
    match Ledger::load() {
        Some(mut l) if !l.is_empty() => {
            let n = l.restore_all();
            let _ = l.save();
            log(&format!("{reason}: restored {n} hidden window(s), styling and taskbar"));
        }
        _ => log(&format!("{reason}: nothing to restore")),
    }
}

fn main() {
    let name = hyprwin_win32::wide("Local\\hyprwin-guard");
    let _m = unsafe { CreateMutexW(None, true, PCWSTR(name.as_ptr())) };
    if unsafe { windows::Win32::Foundation::GetLastError() }.0 == 183 {
        return; // another guard is already watching
    }
    hyprwin_win32::com_init();
    let pid: u32 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let daemon: HANDLE = unsafe { OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, false, pid) }.unwrap_or_default();
    if daemon.is_invalid() {
        log(&format!("cannot open daemon pid {pid}; exiting"));
        return;
    }
    let hotkey = unsafe {
        RegisterHotKey(None, 1, MOD_CONTROL | MOD_ALT | MOD_SHIFT | MOD_WIN | MOD_NOREPEAT, VK_ESCAPE.0 as u32)
    };
    match &hotkey {
        Ok(()) => log(&format!("watching daemon pid {pid}; emergency key Ctrl+Alt+Shift+Win+Esc armed")),
        Err(e) => log(&format!("watching daemon pid {pid}; WARNING: emergency key unavailable: {e}")),
    }

    loop {
        let r = unsafe { MsgWaitForMultipleObjects(Some(&[daemon]), false, u32::MAX, QS_ALLINPUT) };
        if r == WAIT_OBJECT_0 {
            restore("daemon exited");
            break;
        }
        let mut msg = MSG::default();
        let mut emergency = false;
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_HOTKEY {
                    emergency = true;
                }
                DispatchMessageW(&msg);
            }
        }
        if emergency {
            log("EMERGENCY combo pressed");
            unsafe {
                let _ = TerminateProcess(daemon, 2);
            }
            restore("emergency");
            break;
        }
    }
    hyprwin_win32::cloak::release();
    unsafe {
        let _ = CloseHandle(daemon);
    }
}
