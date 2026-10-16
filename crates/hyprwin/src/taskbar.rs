//! Hiding the Windows taskbar (`hyprwin:hide_taskbar`), Hyprland-style.
//!
//! The taskbar windows are hidden with `SW_HIDE`, which is reversible and
//! leaves the user's taskbar settings alone (unlike auto-hide). Each one is
//! recorded in the ledger *before* it is hidden, so the daemon on exit, the
//! panic hook and the guard all bring it back.
//!
//! Hidden, the tray (including hyprwin's own icon) is out of reach, so the
//! taskbar comes back while the mouse touches the screen edge it lives on, or
//! while the Start menu or a tray flyout is open, and hides again afterwards.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use hyprwin_layout::Rect;
use hyprwin_win32::{HWND, dwm, window};
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, IsWindowVisible, SW_HIDE, SW_SHOWNA, ShowWindow};

/// Taskbars hidden right now, with the rect each had (for the work area and
/// the reveal strip). Read by `monitor_specs`.
static HIDDEN: Mutex<Vec<(isize, Rect)>> = Mutex::new(Vec::new());

/// How long the mouse may be away before a revealed taskbar hides again.
const LEAVE_DELAY: Duration = Duration::from_millis(500);

pub fn find() -> Vec<HWND> {
    window::top_level_windows().into_iter().filter(|&h| matches!(window::class_name(h).as_str(), "Shell_TrayWnd" | "Shell_SecondaryTrayWnd")).collect()
}

/// Rects of the taskbars hyprwin has hidden.
pub fn hidden_rects() -> Vec<Rect> {
    HIDDEN.lock().map(|v| v.iter().map(|(_, r)| *r).collect()).unwrap_or_default()
}

/// Gives back to `work` the edge of `mon` a hidden taskbar was occupying.
pub fn reclaim(mon: Rect, mut work: Rect) -> Rect {
    for b in hidden_rects() {
        let (cx, cy) = (b.x + b.w / 2, b.y + b.h / 2);
        if cx < mon.x || cx >= mon.right() || cy < mon.y || cy >= mon.bottom() {
            continue;
        }
        if b.w >= b.h {
            if cy > mon.y + mon.h / 2 {
                work.h = mon.bottom() - work.y; // bottom
            } else {
                work.h += work.y - mon.y; // top
                work.y = mon.y;
            }
        } else if cx > mon.x + mon.w / 2 {
            work.w = mon.right() - work.x; // right
        } else {
            work.w += work.x - mon.x; // left
            work.x = mon.x;
        }
    }
    work
}

pub fn is_hidden() -> bool {
    HIDDEN.lock().map(|v| !v.is_empty()).unwrap_or(false)
}

/// Hides every taskbar. `record` stores the HWNDs in the ledger and saves it;
/// it runs before anything is hidden.
pub fn hide_all(record: &mut dyn FnMut(&[isize])) -> usize {
    let bars: Vec<(isize, Rect)> = find()
        .into_iter()
        .filter_map(|h| {
            // The visible frame of a hidden bar is empty; use the window rect.
            let r = window::window_rect(h).ok().or_else(|| dwm::frame_bounds(h).ok())?;
            Some((h.0 as isize, r))
        })
        .collect();
    if bars.is_empty() {
        return 0;
    }
    record(&bars.iter().map(|(h, _)| *h).collect::<Vec<_>>());
    for (h, _) in &bars {
        unsafe {
            let _ = ShowWindow(HWND(*h as *mut _), SW_HIDE);
        }
    }
    let n = bars.len();
    if let Ok(mut v) = HIDDEN.lock() {
        *v = bars;
    }
    n
}

/// Shows every taskbar hyprwin hid. Returns the HWNDs (to drop from the ledger).
pub fn show_all() -> Vec<isize> {
    let bars = HIDDEN.lock().map(|mut v| std::mem::take(&mut *v)).unwrap_or_default();
    for (h, _) in &bars {
        unsafe {
            let _ = ShowWindow(HWND(*h as *mut _), SW_SHOWNA);
        }
    }
    bars.into_iter().map(|(h, _)| h).collect()
}

/// The thin strip along the screen edge where a hidden taskbar lives.
fn edge_strip(b: Rect) -> Rect {
    const T: i32 = 2;
    let mon = hyprwin_win32::monitor::monitors().into_iter().map(|m| m.rect).find(|m| {
        let (cx, cy) = (b.x + b.w / 2, b.y + b.h / 2);
        cx >= m.x && cx < m.right() && cy >= m.y && cy < m.bottom()
    });
    let Some(m) = mon else { return Rect::new(0, 0, 0, 0) };
    if b.w >= b.h {
        if b.y + b.h / 2 > m.y + m.h / 2 { Rect::new(b.x, m.bottom() - T, b.w, T) } else { Rect::new(b.x, m.y, b.w, T) }
    } else if b.x + b.w / 2 > m.x + m.w / 2 {
        Rect::new(m.right() - T, b.y, T, b.h)
    } else {
        Rect::new(m.x, b.y, T, b.h)
    }
}

fn inside(r: Rect, x: i32, y: i32) -> bool {
    x >= r.x && x < r.right() && y >= r.y && y < r.bottom()
}

/// Shell UI that should keep a revealed taskbar up while it is open.
fn shell_ui_open() -> bool {
    let fg = unsafe { GetForegroundWindow() };
    matches!(
        window::class_name(fg).as_str(),
        "Shell_TrayWnd"
            | "Shell_SecondaryTrayWnd"
            | "Windows.UI.Core.CoreWindow"
            | "XamlExplorerHostIslandWindow"
            | "NotifyIconOverflowWindow"
            | "TopLevelWindowForOverflowXamlIsland"
            | "#32768" // a tray icon's context menu
    )
}

/// Reveal-on-edge state.
#[derive(Default)]
pub struct Reveal {
    shown: bool,
    away_since: Option<Instant>,
}

impl Reveal {
    /// Mouse moved to (x, y). Returns true if the taskbar was just revealed
    /// (the caller then polls [`tick`](Self::tick) until it hides again).
    pub fn on_mouse(&mut self, x: i32, y: i32) -> bool {
        if self.shown || !is_hidden() {
            return false;
        }
        let bars = HIDDEN.lock().map(|v| v.clone()).unwrap_or_default();
        if bars.iter().any(|(_, r)| inside(edge_strip(*r), x, y)) {
            for (h, _) in &bars {
                unsafe {
                    let _ = ShowWindow(HWND(*h as *mut _), SW_SHOWNA);
                }
            }
            self.shown = true;
            self.away_since = None;
            return true;
        }
        false
    }

    /// The taskbar showed itself (Start menu, Win+T…): treat it as revealed.
    pub fn on_shown_by_system(&mut self) -> bool {
        if self.shown || !is_hidden() {
            return false;
        }
        self.shown = true;
        self.away_since = None;
        true
    }

    /// Called periodically while revealed. Returns false once hidden again.
    pub fn tick(&mut self) -> bool {
        if !self.shown {
            return false;
        }
        if !is_hidden() {
            self.shown = false;
            return false;
        }
        let mut p = POINT::default();
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut p);
        }
        let bars = HIDDEN.lock().map(|v| v.clone()).unwrap_or_default();
        let over = bars.iter().any(|(_, r)| inside(Rect::new(r.x - 2, r.y - 2, r.w + 4, r.h + 4), p.x, p.y));
        if over || shell_ui_open() {
            self.away_since = None;
            return true;
        }
        let since = *self.away_since.get_or_insert_with(Instant::now);
        if since.elapsed() < LEAVE_DELAY {
            return true;
        }
        for (h, _) in &bars {
            unsafe {
                let _ = ShowWindow(HWND(*h as *mut _), SW_HIDE);
            }
        }
        self.shown = false;
        false
    }

    /// True if `h` is one of the hidden taskbars and it is visible now.
    pub fn is_our_bar_visible(h: HWND) -> bool {
        HIDDEN.lock().map(|v| v.iter().any(|(b, _)| *b == h.0 as isize)).unwrap_or(false) && unsafe { IsWindowVisible(h).as_bool() }
    }
}
