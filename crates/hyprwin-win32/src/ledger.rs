//! Crash ledger: the durable record of every change hyprwin makes to other
//! apps' windows, so they can always be put back, even if the daemon is
//! killed (the guard replays it) or crashes (the panic hook replays it).
//!
//! Written atomically (temp file + rename) *before* each hide.

use std::collections::BTreeMap;
use std::path::PathBuf;

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    SW_HIDE, SW_SHOWNA, SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos, ShowWindow,
};

use crate::{Rect, cloak, dwm, window};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HideMethod {
    Cloak,
    /// Moved far off-screen; `Rect` is where it was.
    Park(Rect),
}

#[derive(Clone, Debug, Default)]
pub struct Ledger {
    pub daemon_pid: u32,
    pub hidden: BTreeMap<isize, HideMethod>,
    /// Windows whose DWM border colour / corners we changed.
    pub styled: BTreeMap<isize, ()>,
    /// Taskbar windows we hid.
    pub taskbars: Vec<isize>,
}

/// `%LOCALAPPDATA%\hyprwin`, or `HYPRWIN_STATE_DIR` (used by tests so they
/// never touch a real session's ledger or saved layout).
pub fn state_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("HYPRWIN_STATE_DIR") {
        return PathBuf::from(d);
    }
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("hyprwin")
}

pub fn ledger_path() -> PathBuf {
    state_dir().join("ledger.json")
}

impl Ledger {
    pub fn to_json(&self) -> String {
        let hidden: Vec<String> = self
            .hidden
            .iter()
            .map(|(h, m)| match m {
                HideMethod::Cloak => format!("{{\"hwnd\":{h},\"method\":\"cloak\"}}"),
                HideMethod::Park(r) => format!("{{\"hwnd\":{h},\"method\":\"park\",\"rect\":[{},{},{},{}]}}", r.x, r.y, r.w, r.h),
            })
            .collect();
        let styled: Vec<String> = self.styled.keys().map(|h| h.to_string()).collect();
        let taskbars: Vec<String> = self.taskbars.iter().map(|h| h.to_string()).collect();
        format!(
            "{{\"daemon_pid\":{},\"hidden\":[{}],\"styled\":[{}],\"taskbars\":[{}]}}",
            self.daemon_pid,
            hidden.join(","),
            styled.join(","),
            taskbars.join(",")
        )
    }

    pub fn from_json(s: &str) -> Option<Ledger> {
        let v: serde_json::Value = serde_json::from_str(s).ok()?;
        let mut l = Ledger { daemon_pid: v["daemon_pid"].as_u64().unwrap_or(0) as u32, ..Default::default() };
        for e in v["hidden"].as_array()? {
            let h = e["hwnd"].as_i64()? as isize;
            let m = if e["method"] == "park" {
                let r = e["rect"].as_array()?;
                let n = |i: usize| r.get(i).and_then(|x| x.as_i64()).unwrap_or(0) as i32;
                HideMethod::Park(Rect::new(n(0), n(1), n(2), n(3)))
            } else {
                HideMethod::Cloak
            };
            l.hidden.insert(h, m);
        }
        for h in v["styled"].as_array().into_iter().flatten() {
            if let Some(h) = h.as_i64() {
                l.styled.insert(h as isize, ());
            }
        }
        for h in v["taskbars"].as_array().into_iter().flatten() {
            if let Some(h) = h.as_i64() {
                l.taskbars.push(h as isize);
            }
        }
        Some(l)
    }

    pub fn load() -> Option<Ledger> {
        Self::from_json(&std::fs::read_to_string(ledger_path()).ok()?)
    }

    pub fn save(&self) -> std::io::Result<()> {
        let dir = state_dir();
        std::fs::create_dir_all(&dir)?;
        let tmp = dir.join("ledger.json.tmp");
        std::fs::write(&tmp, self.to_json())?;
        std::fs::rename(&tmp, ledger_path())
    }

    pub fn is_empty(&self) -> bool {
        self.hidden.is_empty() && self.styled.is_empty() && self.taskbars.is_empty()
    }

    /// Puts every window back. Safe to call from any thread, repeatedly.
    pub fn restore_all(&mut self) -> usize {
        crate::com_init();
        let mut n = 0;
        for (&h, m) in &self.hidden {
            let hwnd = HWND(h as *mut _);
            if !window::is_alive(hwnd) {
                continue;
            }
            match m {
                HideMethod::Cloak => {
                    let _ = cloak::set_cloaked(hwnd, false);
                }
                HideMethod::Park(r) => unsafe {
                    let _ = SetWindowPos(hwnd, None, r.x, r.y, r.w, r.h, SWP_NOZORDER | SWP_NOACTIVATE);
                },
            }
            n += 1;
        }
        for &h in self.styled.keys() {
            let hwnd = HWND(h as *mut _);
            if window::is_alive(hwnd) {
                let _ = dwm::set_border_color(hwnd, None);
                let _ = dwm::set_corners(hwnd, dwm::Corners::Default);
            }
        }
        for &h in &self.taskbars {
            unsafe {
                let _ = ShowWindow(HWND(h as *mut _), SW_SHOWNA);
            }
        }
        self.hidden.clear();
        self.styled.clear();
        self.taskbars.clear();
        n
    }
}

/// Hides a taskbar window and returns it for the ledger.
pub fn hide_taskbar_window(h: HWND) {
    unsafe {
        let _ = ShowWindow(h, SW_HIDE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_roundtrip() {
        let mut l = Ledger { daemon_pid: 42, ..Default::default() };
        l.hidden.insert(100, HideMethod::Cloak);
        l.hidden.insert(200, HideMethod::Park(Rect::new(1, 2, 3, 4)));
        l.styled.insert(300, ());
        l.taskbars.push(400);
        let back = Ledger::from_json(&l.to_json()).unwrap();
        assert_eq!(back.daemon_pid, 42);
        assert_eq!(back.hidden, l.hidden);
        assert_eq!(back.styled.len(), 1);
        assert_eq!(back.taskbars, vec![400]);
    }
}
