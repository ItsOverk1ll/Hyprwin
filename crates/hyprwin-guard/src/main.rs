//! hyprwin-guard: the safety net.
//!
//! - Owns the hard-coded emergency combo **Ctrl+Alt+Shift+Win+Esc**
//!   (`RegisterHotKey`, so it works even if the daemon's hooks are hung).
//!   Pressing it restores every window and the taskbar and kills the daemon.
//! - Waits on the daemon process. If it dies without cleaning up, replays the
//!   ledger so no window is ever left hidden.

#![windows_subsystem = "windows"]

use hyprwin_win32::ledger::Ledger;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{
    CreateMutexW, GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, TerminateProcess,
    WaitForSingleObject,
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

/// Replays the ledger, if it is `pid`'s. After a restart the ledger may
/// already belong to the new daemon, whose hidden windows must stay hidden.
fn restore(reason: &str, pid: u32) {
    match Ledger::load() {
        Some(l) if l.daemon_pid != pid && l.daemon_pid != 0 => {
            log(&format!("{reason}: the ledger belongs to pid {} now; leaving it alone", l.daemon_pid));
        }
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
    let m = unsafe { CreateMutexW(None, true, PCWSTR(name.as_ptr())) }.unwrap_or_default();
    if unsafe { windows::Win32::Foundation::GetLastError() }.0 == 183 {
        // Another guard holds the lock. After a restart (an update) that is
        // usually the previous daemon's guard, still restoring before it
        // exits: wait for it rather than leave this daemon unguarded.
        let r = unsafe { WaitForSingleObject(m, 10_000) };
        if r != WAIT_OBJECT_0 && r != WAIT_ABANDONED {
            return; // a guard is still watching
        }
    }
    hyprwin_win32::com_init();
    let pid: u32 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let daemon: HANDLE = unsafe { OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.unwrap_or_default();
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
            let mut code = 0u32;
            let _ = unsafe { GetExitCodeProcess(daemon, &mut code) };
            restore("daemon exited", pid);
            // 0 is a normal exit and 101 a panic (the daemon wrote that report
            // itself). Anything else: killed, or crashed hard (no panic hook).
            let just_reported = hyprwin_win32::crash::reports().first().and_then(|p| p.metadata().ok()?.modified().ok()?.elapsed().ok()).is_some_and(|age| age.as_secs() < 10);
            if code != 0 && code != 101 && !just_reported {
                log(&format!("daemon pid {pid} stopped unexpectedly (exit code {code:#x})"));
                hyprwin_win32::crash::write_report(
                    "stopped unexpectedly",
                    &format!("hyprwin (pid {pid}) stopped without shutting down normally.\nexit code: {code} ({code:#x})\n\nCommon codes: 0xc0000005 = crashed (access violation), 0xc00000fd = stack overflow, 1 = ended by Task Manager or another program."),
                );
            }
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
            hyprwin_win32::crash::write_report("emergency key", "The emergency key (Ctrl+Alt+Shift+Win+Esc) was pressed: something was probably wrong. The log below shows what hyprwin was doing just before.");
            unsafe {
                let _ = TerminateProcess(daemon, 2);
            }
            restore("emergency", pid);
            break;
        }
    }
    hyprwin_win32::cloak::release();
    unsafe {
        let _ = CloseHandle(daemon);
    }
}
