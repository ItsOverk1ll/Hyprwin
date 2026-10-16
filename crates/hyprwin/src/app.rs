//! The daemon's main-thread state: owns the WM model, translates Windows
//! events into model operations and model effects into Win32 calls.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use hyprwin_config::{Cmd, Config, Delta, RuleEffect, WindowProps};
use hyprwin_layout::wm::{Anim, MonitorSpec, NewWindow};
use hyprwin_layout::{Dir, Effect, FullscreenMode, Rect, WinId, Wm, WsSel};
use hyprwin_win32::ledger::{HideMethod, Ledger};
use hyprwin_win32::{HWND, cloak, dwm, monitor, window};
use serde_json::json;
use windows::Win32::Foundation::{LPARAM, POINT, WPARAM};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::classify::{self, Meta, Verdict};
use crate::input;
use crate::{debug, info, warn};

pub const TIMER_VERIFY: usize = 1;
pub const TIMER_MONITORS: usize = 2;

const VERIFY_DELAY_MS: u32 = 150;
const FIGHT_LIMIT: u32 = 4;
const FIGHT_WINDOW: Duration = Duration::from_secs(5);

struct Drag {
    id: WinId,
    resize: bool,
    start_rect: Rect,
    start: (i32, i32),
    last: (i32, i32),
    left: bool,
    top: bool,
    tiled: bool,
}

pub struct App {
    pub cfg: Config,
    pub cfg_path: PathBuf,
    pub wm: Wm,
    meta: HashMap<WinId, Meta>,
    initial: HashMap<WinId, (String, String)>,
    pub ledger: Ledger,
    main: HWND,
    own_pid: u32,
    binds: Vec<hyprwin_config::Bind>,
    submaps: HashMap<String, u32>,
    placed_at: HashMap<WinId, Instant>,
    fights: HashMap<WinId, (u32, Instant)>,
    stubborn: HashSet<WinId>,
    in_movesize: HashSet<WinId>,
    pending_min: HashMap<WinId, (i32, i32)>,
    drag: Option<Drag>,
    last_hover: isize,
    border_override: HashMap<WinId, u32>,
    events: Sender<String>,
    verify_pending: bool,
    pub quit: bool,
    pub started: Instant,
}

fn hwnd(id: WinId) -> HWND {
    HWND(id as isize as *mut _)
}

fn id_of(h: HWND) -> WinId {
    h.0 as isize as WinId
}

pub fn monitor_specs(cfg: &Config) -> Vec<MonitorSpec> {
    let mut mons = monitor::monitors();
    // Primary first so it gets workspace 1, then left-to-right.
    mons.sort_by_key(|m| (!m.primary, m.rect.x, m.rect.y));
    mons.into_iter()
        .map(|m| {
            let name = hyprwin_config::model::normalize_monitor(&m.device);
            let mut work = m.work;
            if let Some(&(t, b, l, r)) = cfg.reserved.get(&name) {
                work = Rect::new(work.x + l, work.y + t, work.w - l - r, work.h - t - b);
            }
            MonitorSpec { name, rect: m.rect, work, scale: m.scale() }
        })
        .collect()
}

impl App {
    pub fn new(cfg: Config, cfg_path: PathBuf, main: HWND, events: Sender<String>) -> App {
        let wm = Wm::new(cfg.wm.clone(), monitor_specs(&cfg));
        let mut app = App {
            cfg,
            cfg_path,
            wm,
            meta: HashMap::new(),
            initial: HashMap::new(),
            ledger: Ledger { daemon_pid: std::process::id(), ..Default::default() },
            main,
            own_pid: std::process::id(),
            binds: Vec::new(),
            submaps: HashMap::new(),
            placed_at: HashMap::new(),
            fights: HashMap::new(),
            stubborn: HashSet::new(),
            in_movesize: HashSet::new(),
            pending_min: HashMap::new(),
            drag: None,
            last_hover: 0,
            border_override: HashMap::new(),
            events,
            verify_pending: false,
            quit: false,
            started: Instant::now(),
        };
        app.install_binds();
        let _ = app.ledger.save();
        app
    }

    fn install_binds(&mut self) {
        self.binds = self.cfg.effective_binds();
        self.submaps.clear();
        for b in &self.binds {
            if let Some(s) = &b.submap {
                let n = self.submaps.len() as u32 + 1;
                self.submaps.entry(s.clone()).or_insert(n);
            }
        }
        input::set_binds(&self.binds, &self.submaps);
        input::shared().follow_mouse.store(self.cfg.follow_mouse != 0, std::sync::atomic::Ordering::Relaxed);
    }

    /// Manages every existing window, bottom of the z-order first so the
    /// top-most window ends up focused.
    pub fn manage_existing(&mut self) {
        let top_first = window::top_level_windows();
        for &h in top_first.iter().rev() {
            self.try_manage(h);
        }
        // The top of the z-order is what the user used last: treat it as the
        // oldest, so overflow moves the least recently used windows first.
        let order: Vec<WinId> = top_first.iter().map(|&h| id_of(h)).collect();
        self.wm.set_age_order(&order);
        if let Some(fg) = Some(unsafe { GetForegroundWindow() }).filter(|h| !h.is_invalid()) {
            let fx = self.wm.on_os_focus(id_of(fg));
            self.apply(fx);
        }
        info!("managing {} window(s) on {} monitor(s)", self.wm.windows.len(), self.wm.monitors.len());
    }

    // ------------------------------------------------------------------ manage

    fn props_for(&self, id: WinId, m: &Meta) -> WindowProps {
        let (ic, it) = self.initial.get(&id).cloned().unwrap_or_else(|| (m.class.clone(), m.title.clone()));
        WindowProps {
            class: m.class.clone(),
            title: m.title.clone(),
            initial_class: ic,
            initial_title: it,
            winclass: m.winclass.clone(),
            exe: m.exe.clone(),
            floating: false,
            fullscreen: false,
            pinned: false,
            focused: false,
            workspace: String::new(),
        }
    }

    pub fn try_manage(&mut self, h: HWND) {
        let id = id_of(h);
        if self.wm.windows.contains_key(&id) {
            return;
        }
        let m = classify::read_meta(h);
        // Test hook: only manage windows whose title matches (used by tests/live).
        if let Some(re) = manage_only() {
            if !re.is_match(&m.title) {
                return;
            }
        }
        let verdict = classify::classify(h, &m, self.own_pid);
        let Verdict::Manage { float, reason } = verdict else {
            if let Verdict::Ignore(why) = verdict {
                if m.elevated && window::is_visible(h) && !m.title.is_empty() {
                    info!("not managing elevated window '{}' ({}): {why}", m.title, m.class);
                }
            }
            return;
        };
        self.initial.entry(id).or_insert_with(|| (m.class.clone(), m.title.clone()));
        let props = self.props_for(id, &m);
        let mut nw = NewWindow {
            rect: dwm::frame_bounds(h).unwrap_or_default(),
            floating: float,
            cursor: Some(cursor_pos()).map(|(x, y)| (x as f64, y as f64)),
            ..Default::default()
        };
        let mut fullscreen_mode: Option<FullscreenMode> = None;
        let mut rounding = self.cfg.rounding;
        let mut size: Option<(Delta, Delta)> = None;
        let mut pos: Option<(Delta, Delta)> = None;
        for rule in self.cfg.window_rules.iter().filter(|r| r.matches(&props)) {
            for e in &rule.effects {
                match e {
                    RuleEffect::Unmanaged => {
                        debug!("rule {}: leaving '{}' unmanaged", rule.source, m.title);
                        return;
                    }
                    RuleEffect::Float => nw.floating = true,
                    RuleEffect::Tile => nw.floating = false,
                    RuleEffect::Fullscreen => fullscreen_mode = Some(FullscreenMode::Fullscreen),
                    RuleEffect::Maximize => fullscreen_mode = Some(FullscreenMode::Maximized),
                    RuleEffect::Pin => nw.pin = true,
                    RuleEffect::Pseudo => nw.pseudo = true,
                    RuleEffect::Center => nw.center = true,
                    RuleEffect::Size(x, y) => size = Some((*x, *y)),
                    RuleEffect::Move(x, y) => pos = Some((*x, *y)),
                    RuleEffect::Workspace { ws, silent } => {
                        nw.workspace = WsSel::parse(ws);
                        nw.no_initial_focus |= *silent;
                    }
                    RuleEffect::Monitor(name) => {
                        nw.monitor = self.wm.monitors.iter().position(|mm| &mm.spec.name == name).or_else(|| name.parse().ok());
                    }
                    RuleEffect::NoInitialFocus | RuleEffect::NoFocus => nw.no_initial_focus = true,
                    RuleEffect::Rounding(r) => rounding = *r,
                    RuleEffect::BorderColor(c) => {
                        self.border_override.insert(id, c.rgb());
                    }
                    RuleEffect::Opacity(_) | RuleEffect::NoAnim | RuleEffect::Animation(_) => {}
                }
            }
        }
        if size.is_some() || pos.is_some() {
            let mon = nw.monitor.unwrap_or(self.wm.focused_monitor).min(self.wm.monitors.len() - 1);
            let mr = self.wm.monitors[mon].spec.rect;
            let resolve = |d: Delta, total: i32| if d.percent { (total as f64 * d.value / 100.0) as i32 } else { d.value as i32 };
            let mut r = nw.rect;
            if let Some((w, hh)) = size {
                r.w = resolve(w, mr.w);
                r.h = resolve(hh, mr.h);
            }
            if let Some((x, y)) = pos {
                r.x = mr.x + resolve(x, mr.w);
                r.y = mr.y + resolve(y, mr.h);
            }
            nw.float_rect = Some(r);
        }
        if fullscreen_mode == Some(FullscreenMode::Fullscreen) {
            nw.fullscreen = true;
        }
        info!("manage {:#x} '{}' class={} winclass={} {} ({reason})", id, m.title, m.class, m.winclass, if nw.floating { "float" } else { "tile" });
        // Style first, recorded in the ledger so it is always undone.
        let _ = dwm::set_corners(h, dwm::Corners::from_hypr_rounding(rounding));
        self.ledger.styled.insert(h.0 as isize, ());
        let _ = self.ledger.save();
        self.meta.insert(id, m);
        let fx = self.wm.add_window(id, nw);
        self.apply(fx);
        if fullscreen_mode == Some(FullscreenMode::Maximized) {
            let fx = self.wm.fullscreen(FullscreenMode::Maximized, Some(true));
            self.apply(fx);
        }
        self.set_border(id, self.wm.focus == Some(id));
    }

    fn unmanage(&mut self, id: WinId) {
        if !self.wm.windows.contains_key(&id) {
            return;
        }
        let h = hwnd(id);
        if self.ledger.hidden.contains_key(&(h.0 as isize)) {
            self.reveal(id);
        }
        if self.ledger.styled.remove(&(h.0 as isize)).is_some() && window::is_alive(h) {
            let _ = dwm::set_border_color(h, None);
            let _ = dwm::set_corners(h, dwm::Corners::Default);
        }
        let _ = self.ledger.save();
        debug!("unmanage {:#x}", id);
        for m in [&mut self.placed_at] {
            m.remove(&id);
        }
        self.fights.remove(&id);
        self.pending_min.remove(&id);
        self.stubborn.remove(&id);
        self.meta.remove(&id);
        self.border_override.remove(&id);
        let fx = self.wm.remove_window(id);
        self.apply(fx);
    }

    // ------------------------------------------------------------------ effects

    pub fn apply(&mut self, fx: Vec<Effect>) {
        let mut ledger_dirty = false;
        for e in fx {
            match e {
                Effect::Place { id, rect, anim } => self.place(id, rect, anim),
                Effect::Hide(id) => {
                    self.hide(id);
                    ledger_dirty = true;
                }
                Effect::Show(id) => {
                    self.reveal(id);
                    ledger_dirty = true;
                }
                Effect::Focus(id) => self.focus(id),
                Effect::Raise(id) => unsafe {
                    let _ = SetWindowPos(hwnd(id), Some(HWND_TOP), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS);
                },
                Effect::Close(id) => unsafe {
                    let _ = PostMessageW(Some(hwnd(id)), WM_CLOSE, WPARAM(0), LPARAM(0));
                },
                Effect::Kill(id) => {
                    if let Some(m) = self.meta.get(&id) {
                        warn!("force-killing pid {} ({})", m.pid, m.class);
                        unsafe {
                            if let Ok(p) = OpenProcess(PROCESS_TERMINATE, false, m.pid) {
                                let _ = TerminateProcess(p, 1);
                            }
                        }
                    }
                }
                Effect::SetFullscreen { id, on } => crate::shell::mark_fullscreen(hwnd(id), on),
                Effect::Border { id, active } => self.set_border(id, active),
                Effect::WarpCursor { x, y } => unsafe {
                    let _ = windows::Win32::UI::WindowsAndMessaging::SetCursorPos(x, y);
                },
                Effect::Event { name, data } => self.emit(name, &data),
            }
        }
        if ledger_dirty {
            if let Err(e) = self.ledger.save() {
                warn!("cannot save ledger: {e}");
            }
        }
    }

    fn emit(&self, name: &str, data: &str) {
        let _ = self.events.send(format!("{name}>>{data}\n"));
    }

    fn place(&mut self, id: WinId, rect: Rect, _anim: Anim) {
        if self.stubborn.contains(&id) {
            return;
        }
        let h = hwnd(id);
        unsafe {
            if IsZoomed(h).as_bool() || IsIconic(h).as_bool() && !self.wm.windows.get(&id).is_some_and(|w| w.minimized) {
                let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
                let _ = ShowWindow(h, SW_RESTORE);
            }
        }
        if dwm::frame_bounds(h).ok() == Some(rect) {
            return; // never send no-op moves (they make some apps flicker)
        }
        // Parked windows are moved back by the next Show; skip until then.
        if matches!(self.ledger.hidden.get(&(h.0 as isize)), Some(HideMethod::Park(_))) {
            return;
        }
        if let Err(e) = window::set_visible_rect(h, rect) {
            debug!("place {:#x} failed: {e}", id);
        }
        self.placed_at.insert(id, Instant::now());
        self.schedule_verify();
    }

    fn hide(&mut self, id: WinId) {
        let h = hwnd(id);
        let key = h.0 as isize;
        if self.ledger.hidden.contains_key(&key) {
            return;
        }
        // Record first, then act: a crash in between still restores it.
        self.ledger.hidden.insert(key, HideMethod::Cloak);
        let _ = self.ledger.save();
        if let Err(e) = cloak::set_cloaked(h, true) {
            let r = window::window_rect(h).unwrap_or_default();
            debug!("cloak {:#x} failed ({e}); parking off-screen", id);
            self.ledger.hidden.insert(key, HideMethod::Park(r));
            let _ = self.ledger.save();
            let far = self.wm.monitors.iter().map(|m| m.spec.rect.right()).max().unwrap_or(0) + 4000;
            unsafe {
                let _ = SetWindowPos(h, None, far, r.y, r.w, r.h, SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS);
            }
        }
    }

    fn reveal(&mut self, id: WinId) {
        let h = hwnd(id);
        match self.ledger.hidden.remove(&(h.0 as isize)) {
            Some(HideMethod::Cloak) => {
                let _ = cloak::set_cloaked(h, false);
            }
            Some(HideMethod::Park(r)) => {
                let target = self.wm.desired_rect(id);
                match target {
                    Some(t) => {
                        let _ = window::set_visible_rect(h, t);
                    }
                    None => unsafe {
                        let _ = SetWindowPos(h, None, r.x, r.y, r.w, r.h, SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS);
                    },
                }
            }
            None => {}
        }
    }

    fn focus(&mut self, id: WinId) {
        let h = hwnd(id);
        unsafe {
            if IsIconic(h).as_bool() {
                return;
            }
            if GetForegroundWindow() != h {
                // The foreground lock only lets the process that received the
                // last input change focus: inject a no-op key first.
                crate::shell::inject_noop();
                let _ = SetForegroundWindow(h);
            }
        }
        self.keep_floating_on_top(id);
    }

    /// Hyprland keeps floating windows above tiled ones. Activating a tiled
    /// window on Windows raises it to the top, so slot it directly *below* the
    /// lowest visible floating window instead. The request is queued on the
    /// tiled window's own thread, after its activation, so it cannot race.
    fn keep_floating_on_top(&self, focused: WinId) {
        let Some(w) = self.wm.windows.get(&focused) else { return };
        if w.floating || self.wm.fullscreen_of(focused).is_some() {
            return;
        }
        let mon = self.wm.workspaces.get(&w.ws).map(|x| x.monitor);
        let is_visible_float = |h: HWND| {
            self.wm.windows.get(&id_of(h)).is_some_and(|f| {
                f.floating && !f.minimized && self.wm.desired_rect(f.id).is_some() && self.wm.workspaces.get(&f.ws).map(|x| x.monitor) == mon
            })
        };
        // Top-level windows are enumerated top to bottom: the last float found is the lowest.
        let Some(lowest) = window::top_level_windows().into_iter().filter(|&h| is_visible_float(h)).last() else { return };
        unsafe {
            let _ = SetWindowPos(hwnd(focused), Some(lowest), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS);
        }
    }

    fn set_border(&self, id: WinId, active: bool) {
        if self.cfg.border_size <= 0 {
            return;
        }
        let c = if active { self.border_override.get(&id).copied().unwrap_or(self.cfg.col_active.rgb()) } else { self.cfg.col_inactive.rgb() };
        let _ = dwm::set_border_color(hwnd(id), Some(c));
    }

    // ------------------------------------------------------------------ verify / fights

    fn schedule_verify(&mut self) {
        if !self.verify_pending {
            self.verify_pending = true;
            unsafe {
                SetTimer(Some(self.main), TIMER_VERIFY, VERIFY_DELAY_MS, None);
            }
        }
    }

    /// Re-checks managed windows after the dust settles: apps that moved
    /// themselves are put back (a few times), maximize becomes fullscreen 1,
    /// and windows with a minimum size larger than their tile are tolerated.
    pub fn verify(&mut self) {
        self.verify_pending = false;
        unsafe {
            let _ = KillTimer(Some(self.main), TIMER_VERIFY);
        }
        let ids: Vec<WinId> = self.wm.windows.keys().copied().collect();
        let now = Instant::now();
        for id in ids {
            let h = hwnd(id);
            if !window::is_alive(h) {
                self.unmanage(id);
                continue;
            }
            if self.in_movesize.contains(&id) || self.drag.as_ref().is_some_and(|d| d.id == id) || self.stubborn.contains(&id) {
                continue;
            }
            let Some(desired) = self.wm.desired_rect(id) else { continue };
            if self.ledger.hidden.contains_key(&(h.0 as isize)) {
                continue;
            }
            if unsafe { IsZoomed(h).as_bool() } && !self.wm.windows[&id].floating {
                info!("{:#x} maximized itself: treating as fullscreen 1", id);
                let fx = self.wm.focus_window(id);
                self.apply(fx);
                let fx = self.wm.fullscreen(FullscreenMode::Maximized, Some(true));
                self.apply(fx);
                unsafe {
                    let _ = ShowWindowAsync(h, SW_RESTORE);
                }
                self.wm.invalidate(id);
                continue;
            }
            let Ok(actual) = dwm::frame_bounds(h) else { continue };
            if actual == desired || near(actual, desired) {
                continue;
            }
            if self.wm.windows[&id].floating {
                // The user (or app) moved a floating window: accept it.
                let fx = self.wm.on_user_move(id, actual, actual.center());
                self.apply(fx);
                continue;
            }
            let bigger = actual.w > desired.w + 1 || actual.h > desired.h + 1;
            // A real minimum-size refusal: Windows applies the new position but
            // clamps the size. A window that is merely slow to resize is still at
            // its old position, so it must not be mistaken for a minimum.
            let moved_there = (actual.x - desired.x).abs() <= 1 && (actual.y - desired.y).abs() <= 1;
            if bigger && moved_there && self.pending_min.get(&id) != Some(&(actual.w, actual.h)) {
                // First sighting: ask once more and only learn if it refuses again.
                self.pending_min.insert(id, (actual.w, actual.h));
                self.wm.invalidate(id);
                let fx = self.wm.sync();
                self.apply(fx);
                continue;
            }
            if bigger && moved_there {
                self.pending_min.remove(&id);
                // The app refused to shrink twice: learn its minimum and re-tile around it.
                let known = self.wm.windows[&id].min_size;
                let min = (if actual.w > desired.w + 1 { actual.w } else { known.0 }, if actual.h > desired.h + 1 { actual.h } else { known.1 });
                if min != known {
                    info!("{:#x} ({}) has a minimum size of {}x{}; re-tiling", id, self.meta.get(&id).map(|m| m.class.as_str()).unwrap_or("?"), min.0, min.1);
                    let fx = self.wm.set_min_size(id, min.0, min.1);
                    self.apply(fx);
                    // Still does not fit (too many windows for the screen): float it.
                    if let Some(d) = self.wm.desired_rect(id) {
                        if d.w + 2 < min.0 || d.h + 2 < min.1 {
                            let ws = self.wm.windows[&id].ws;
                            let (fx, moved) = self.wm.overflow_ws(ws);
                            for (w, to) in moved {
                                match to {
                                    Some(n) => info!("{:#x} has no room on workspace {ws}; moved it to workspace {n}", w),
                                    None => warn!("{:#x} fits on no workspace (larger than the screen); floating it", w),
                                }
                            }
                            self.apply(fx);
                        }
                    }
                    continue;
                }
            }
            if self.placed_at.get(&id).is_some_and(|t| now.duration_since(*t) < Duration::from_millis(VERIFY_DELAY_MS as u64)) {
                self.schedule_verify();
                continue;
            }
            let entry = self.fights.entry(id).or_insert((0, now));
            if now.duration_since(entry.1) > FIGHT_WINDOW {
                *entry = (0, now);
            }
            entry.0 += 1;
            if entry.0 > FIGHT_LIMIT {
                warn!("{:#x} ({}) keeps repositioning itself; floating it", id, self.meta.get(&id).map(|m| m.class.as_str()).unwrap_or("?"));
                self.stubborn.insert(id);
                let fx = self.wm.set_floating(Some(id), Some(true));
                self.stubborn.remove(&id);
                self.apply(fx);
                continue;
            }
            debug!("{:#x} drifted to {:?}, restoring {:?}", id, actual, desired);
            self.wm.invalidate(id);
            let fx = self.wm.sync();
            self.apply(fx);
        }
    }

    // ------------------------------------------------------------------ events

    pub fn on_winevent(&mut self, ev: u32, h: HWND) {
        let id = id_of(h);
        let managed = self.wm.windows.contains_key(&id);
        let ours = self.ledger.hidden.contains_key(&(h.0 as isize));
        match ev {
            EVENT_OBJECT_SHOW | EVENT_OBJECT_CREATE => {
                if !managed {
                    self.try_manage(h);
                }
            }
            EVENT_OBJECT_UNCLOAKED => {
                if !managed && !ours {
                    self.try_manage(h);
                }
            }
            EVENT_OBJECT_DESTROY => {
                if managed {
                    self.ledger.hidden.remove(&(h.0 as isize));
                    self.ledger.styled.remove(&(h.0 as isize));
                    self.unmanage(id);
                }
            }
            EVENT_OBJECT_HIDE => {
                if managed && !ours && !window::is_visible(h) {
                    self.unmanage(id);
                }
            }
            EVENT_OBJECT_CLOAKED => {
                if managed && !ours && dwm::cloaked_state(h) != 0 {
                    debug!("{:#x} cloaked by the system; unmanaging", id);
                    self.unmanage(id);
                }
            }
            EVENT_SYSTEM_FOREGROUND => {
                if managed {
                    let fx = self.wm.on_os_focus(id);
                    self.apply(fx);
                    self.keep_floating_on_top(id);
                } else {
                    self.try_manage(h);
                }
            }
            EVENT_SYSTEM_MINIMIZESTART => {
                if managed {
                    if ours {
                        // Never leave a minimized window cloaked (glazewm#1358).
                        self.reveal(id);
                        let _ = self.ledger.save();
                    }
                    let fx = self.wm.set_minimized(id, true);
                    self.apply(fx);
                }
            }
            EVENT_SYSTEM_MINIMIZEEND => {
                if managed {
                    let fx = self.wm.set_minimized(id, false);
                    self.apply(fx);
                } else {
                    self.try_manage(h);
                }
            }
            EVENT_SYSTEM_MOVESIZESTART => {
                if managed {
                    self.in_movesize.insert(id);
                }
            }
            EVENT_SYSTEM_MOVESIZEEND => {
                if self.in_movesize.remove(&id) && managed {
                    let Ok(r) = dwm::frame_bounds(h) else { return };
                    let desired = self.wm.desired_rect(id);
                    if desired.is_some_and(|d| !near(d, r)) {
                        let d = desired.unwrap();
                        let (cx, cy) = cursor_pos();
                        let fx = if (r.w - d.w).abs() > 4 || (r.h - d.h).abs() > 4 {
                            self.wm.on_user_resize(id, r)
                        } else {
                            self.wm.on_user_move(id, r, (cx as f64, cy as f64))
                        };
                        self.apply(fx);
                    }
                }
            }
            EVENT_OBJECT_LOCATIONCHANGE => {
                if managed && !self.in_movesize.contains(&id) {
                    self.schedule_verify();
                }
            }
            EVENT_OBJECT_NAMECHANGE => {
                if managed {
                    let t = window::title(h);
                    if let Some(m) = self.meta.get_mut(&id) {
                        if m.title != t {
                            m.title = t.clone();
                            self.emit("windowtitle", &format!("{id:x}"));
                            self.emit("windowtitlev2", &format!("{id:x},{t}"));
                        }
                    }
                } else if window::is_visible(h) {
                    self.try_manage(h);
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------ mouse

    pub fn on_mouse_move(&mut self) {
        let (x, y) = input::last_mouse();
        if self.drag.is_some() {
            self.drag_update(x, y);
            return;
        }
        if self.cfg.follow_mouse == 0 {
            return;
        }
        let root = unsafe { GetAncestor(WindowFromPoint(POINT { x, y }), GA_ROOT) };
        let key = root.0 as isize;
        if key == self.last_hover {
            return;
        }
        self.last_hover = key;
        let id = id_of(root);
        if !self.wm.windows.contains_key(&id) || self.wm.focus == Some(id) {
            return;
        }
        let buttons = unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) as u16 & 0x8000 != 0 || GetAsyncKeyState(VK_RBUTTON.0 as i32) as u16 & 0x8000 != 0 };
        if buttons || crate::shell::menu_active() {
            return;
        }
        // float_switch_override_focus = 0: hovering between tiled and floating does not refocus.
        let focused_floating = self.wm.focus.and_then(|f| self.wm.windows.get(&f)).map(|w| w.floating).unwrap_or(false);
        let target_floating = self.wm.windows[&id].floating;
        if self.cfg.float_switch_override_focus == 0 && focused_floating != target_floating {
            return;
        }
        let fx = self.wm.focus_window(id);
        self.apply(fx);
    }

    pub fn on_drag(&mut self, kind: usize, x: i32, y: i32) {
        if kind == input::DRAG_END {
            if let Some(d) = self.drag.take() {
                let r = dwm::frame_bounds(hwnd(d.id)).unwrap_or(d.start_rect);
                let fx = if d.resize {
                    if d.tiled { self.wm.sync() } else { self.wm.on_user_resize(d.id, r) }
                } else {
                    self.wm.on_user_move(d.id, r, (x as f64, y as f64))
                };
                self.apply(fx);
            }
            return;
        }
        let root = unsafe { GetAncestor(WindowFromPoint(POINT { x, y }), GA_ROOT) };
        let id = id_of(root);
        if !self.wm.windows.contains_key(&id) {
            return;
        }
        let Ok(r) = dwm::frame_bounds(root) else { return };
        let (cx, cy) = r.center();
        let tiled = !self.wm.windows[&id].floating;
        self.drag = Some(Drag {
            id,
            resize: kind == input::DRAG_RESIZE,
            start_rect: r,
            start: (x, y),
            last: (x, y),
            left: (x as f64) < cx,
            top: (y as f64) < cy,
            tiled,
        });
        let fx = self.wm.focus_window(id);
        self.apply(fx);
        unsafe {
            let _ = SetWindowPos(root, Some(HWND_TOP), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }

    fn drag_update(&mut self, x: i32, y: i32) {
        let Some(d) = self.drag.as_mut() else { return };
        let h = hwnd(d.id);
        let (dx, dy) = (x - d.start.0, y - d.start.1);
        if !d.resize {
            let _ = window::set_visible_rect(h, d.start_rect.offset(dx, dy));
            return;
        }
        if d.tiled {
            // Live tiled resize: feed incremental edge movement to the layout.
            let (ix, iy) = (x - d.last.0, y - d.last.1);
            d.last = (x, y);
            let id = d.id;
            let (left, top) = (d.left, d.top);
            if let Some(cur) = self.wm.desired_rect(id) {
                let mut r = cur;
                if left {
                    r.x += ix;
                    r.w -= ix;
                } else {
                    r.w += ix;
                }
                if top {
                    r.y += iy;
                    r.h -= iy;
                } else {
                    r.h += iy;
                }
                let fx = self.wm.on_user_resize(id, r);
                self.apply(fx);
            }
            return;
        }
        let mut r = d.start_rect;
        if d.left {
            r.x += dx;
            r.w -= dx;
        } else {
            r.w += dx;
        }
        if d.top {
            r.y += dy;
            r.h -= dy;
        } else {
            r.h += dy;
        }
        r.w = r.w.max(100);
        r.h = r.h.max(60);
        let _ = window::set_visible_rect(h, r);
    }

    // ------------------------------------------------------------------ commands

    pub fn on_bind(&mut self, idx: usize) {
        let Some(b) = self.binds.get(idx).cloned() else { return };
        debug!("bind {} {:?} -> {:?}", b.mods, b.key, b.cmd);
        if let Err(e) = self.run(&b.cmd) {
            warn!("bind {}: {e}", b.source);
        }
    }

    fn resolve_window(&self, sel: &str) -> Option<WinId> {
        let sel = sel.trim();
        if sel.is_empty() || sel == "activewindow" {
            return self.wm.focus;
        }
        let (k, v) = sel.split_once(':').unwrap_or((sel, ""));
        let re = || regex_lite(v);
        self.wm.windows.keys().copied().find(|id| {
            let m = self.meta.get(id);
            match k {
                "address" => u64::from_str_radix(v.trim_start_matches("0x"), 16).ok() == Some(*id),
                "pid" => m.is_some_and(|m| v.parse() == Ok(m.pid)),
                "class" => m.is_some_and(|m| re().is_some_and(|r| r.is_match(&m.class))),
                "title" => m.is_some_and(|m| re().is_some_and(|r| r.is_match(&m.title))),
                "initialclass" => self.initial.get(id).is_some_and(|i| re().is_some_and(|r| r.is_match(&i.0))),
                "initialtitle" => self.initial.get(id).is_some_and(|i| re().is_some_and(|r| r.is_match(&i.1))),
                "floating" => self.wm.windows[id].floating,
                "tiled" => !self.wm.windows[id].floating,
                _ => m.is_some_and(|m| re_or(sel).is_some_and(|r| r.is_match(&m.class))),
            }
        })
    }

    fn delta_px(&self, d: Delta, horizontal: bool) -> i32 {
        if d.percent {
            let m = &self.wm.monitors[self.wm.focused_monitor].spec.rect;
            ((if horizontal { m.w } else { m.h }) as f64 * d.value / 100.0) as i32
        } else {
            (d.value * self.wm.monitors[self.wm.focused_monitor].spec.scale) as i32
        }
    }

    pub fn run(&mut self, cmd: &Cmd) -> Result<(), String> {
        let fx = match cmd {
            Cmd::Exec(c) | Cmd::ExecRaw(c) => {
                crate::shell::exec(c, &self.cfg.env);
                return Ok(());
            }
            Cmd::KillActive => self.wm.close_active(),
            Cmd::ForceKillActive => self.wm.kill_active(),
            Cmd::CloseWindow(s) => {
                let id = self.resolve_window(s).ok_or("no matching window")?;
                vec![Effect::Close(id)]
            }
            Cmd::KillWindow(s) => {
                let id = self.resolve_window(s).ok_or("no matching window")?;
                vec![Effect::Kill(id)]
            }
            Cmd::Workspace(sel) => self.wm.switch_workspace(sel),
            Cmd::FocusWorkspaceOnCurrentMonitor(sel) => {
                if let Some((id, _)) = self.wm.resolve(sel) {
                    if let Some(ws) = self.wm.workspaces.get_mut(&id) {
                        ws.monitor = self.wm.focused_monitor;
                    }
                }
                self.wm.switch_workspace(sel)
            }
            Cmd::MoveToWorkspace { ws, window, follow } => {
                let w = match window {
                    Some(s) => Some(self.resolve_window(s).ok_or("no matching window")?),
                    None => None,
                };
                self.wm.move_to_workspace(w, ws, *follow)
            }
            Cmd::ToggleFloating(s) => {
                let w = s.as_deref().and_then(|s| self.resolve_window(s));
                self.wm.set_floating(w, None)
            }
            Cmd::SetFloating(s) => {
                let w = s.as_deref().and_then(|s| self.resolve_window(s));
                self.wm.set_floating(w, Some(true))
            }
            Cmd::SetTiled(s) => {
                let w = s.as_deref().and_then(|s| self.resolve_window(s));
                self.wm.set_floating(w, Some(false))
            }
            Cmd::Fullscreen { mode, action } => {
                self.wm.fullscreen(if *mode == 1 { FullscreenMode::Maximized } else { FullscreenMode::Fullscreen }, *action)
            }
            Cmd::Pin(s) => {
                let w = s.as_deref().and_then(|s| self.resolve_window(s));
                self.wm.pin(w)
            }
            Cmd::Pseudo(s) => {
                let w = s.as_deref().and_then(|s| self.resolve_window(s));
                self.wm.pseudo(w)
            }
            Cmd::MoveFocus(d) => self.wm.focus_dir(*d),
            Cmd::MoveWindow(d) => self.wm.move_dir(*d),
            Cmd::MoveWindowToMonitor(m) => {
                let target = self.wm.monitors.iter().position(|x| &x.spec.name == m).or_else(|| match Dir::parse(m) {
                    Some(_) => None,
                    None => m.parse().ok(),
                });
                match (target, Dir::parse(m)) {
                    (_, Some(d)) => self.wm.move_dir(d),
                    (Some(i), _) => {
                        let ws = self.wm.monitors.get(i).map(|x| x.active).ok_or("no such monitor")?;
                        self.wm.move_to_workspace(None, &WsSel::Id(ws), false)
                    }
                    _ => return Err(format!("no monitor '{m}'")),
                }
            }
            Cmd::SwapWindow(d) => self.wm.swap_dir(*d),
            Cmd::CenterWindow => self.wm.center_window(),
            Cmd::ResizeActive { x, y, exact } => {
                let (dx, dy) = (self.delta_px(*x, true), self.delta_px(*y, false));
                self.wm.resize_active(dx, dy, *exact)
            }
            Cmd::MoveActive { x, y, exact } => {
                let (dx, dy) = (self.delta_px(*x, true), self.delta_px(*y, false));
                self.wm.move_active(dx, dy, *exact)
            }
            Cmd::CycleNext { prev, tiled, floating } => self.wm.cycle(!prev, *tiled, *floating),
            Cmd::SwapNext { prev } => self.wm.swap_next(!prev),
            Cmd::FocusWindow(s) => {
                let id = self.resolve_window(s).ok_or("no matching window")?;
                self.wm.focus_window(id)
            }
            Cmd::FocusMonitor(m) => self.wm.focus_monitor(m),
            Cmd::FocusCurrentOrLast | Cmd::FocusUrgentOrLast => self.wm.focus_current_or_last(),
            Cmd::ToggleSpecialWorkspace(n) => self.wm.toggle_special(&WsSel::Special(n.clone())),
            Cmd::LayoutMsg(m) => self.wm.layout_msg(m),
            Cmd::Submap(name) => {
                let id = if name == "reset" { 0 } else { *self.submaps.get(name).unwrap_or(&0) };
                input::shared().submap.store(id, std::sync::atomic::Ordering::Relaxed);
                self.emit("submap", if name == "reset" { "" } else { name });
                return Ok(());
            }
            Cmd::BringActiveToTop => self.wm.focus.map(|f| vec![Effect::Raise(f)]).unwrap_or_default(),
            Cmd::Exit => {
                self.quit = true;
                return Ok(());
            }
            Cmd::Reload => {
                self.reload();
                return Ok(());
            }
            Cmd::MouseMove | Cmd::MouseResize => return Ok(()),
            Cmd::Unsupported(why) => return Err(why.clone()),
        };
        self.apply(fx);
        Ok(())
    }

    pub fn reload(&mut self) {
        let cfg = crate::load_config(&self.cfg_path);
        for w in &cfg.warnings {
            warn!("config: {w}");
        }
        info!("config reloaded from {} ({} binds, {} rules, {} warnings)", self.cfg_path.display(), cfg.binds.len(), cfg.window_rules.len(), cfg.warnings.len());
        self.cfg = cfg;
        self.wm.cfg = self.cfg.wm.clone();
        self.install_binds();
        let ids: Vec<WinId> = self.wm.windows.keys().copied().collect();
        for id in ids {
            self.set_border(id, self.wm.focus == Some(id));
        }
        self.wm.invalidate_all();
        let fx = self.wm.sync();
        self.apply(fx);
        self.emit("configreloaded", "");
    }

    pub fn refresh_monitors(&mut self) {
        let specs = monitor_specs(&self.cfg);
        info!("monitors changed: {}", specs.iter().map(|m| format!("{} {:?}@{}", m.name, m.rect, m.scale)).collect::<Vec<_>>().join(", "));
        let fx = self.wm.set_monitors(specs);
        self.apply(fx);
        self.wm.invalidate_all();
        let fx = self.wm.sync();
        self.apply(fx);
    }

    /// Puts every window back the way we found it.
    pub fn restore_everything(&mut self) {
        let n = self.ledger.restore_all();
        let _ = self.ledger.save();
        info!("restored {n} hidden window(s) and all styling");
    }

    // ------------------------------------------------------------------ IPC

    pub fn handle_request(&mut self, req: &str) -> String {
        if let Some(batch) = req.strip_prefix(hyprwin_ipc::BATCH_PREFIX) {
            return batch.split(';').map(|r| self.handle_request(r.trim())).collect::<Vec<_>>().join("\n");
        }
        let (json, req) = match req.strip_prefix("j/") {
            Some(r) => (true, r),
            None => (false, req),
        };
        let (cmd, rest) = req.split_once(' ').unwrap_or((req, ""));
        match cmd {
            "dispatch" => {
                let (d, a) = rest.split_once(' ').unwrap_or((rest, ""));
                match hyprwin_config::dispatch::parse(d, a).and_then(|c| self.run(&c)) {
                    Ok(()) => "ok".into(),
                    Err(e) => e,
                }
            }
            "reload" => {
                self.reload();
                "ok".into()
            }
            "clients" => self.q_clients(json),
            "activewindow" => self.q_activewindow(json),
            "workspaces" => self.q_workspaces(json, false),
            "activeworkspace" => self.q_workspaces(json, true),
            "monitors" => self.q_monitors(json),
            "binds" => self.q_binds(json),
            "version" => {
                if json {
                    json!({"tag": env!("CARGO_PKG_VERSION"), "flags": [], "hyprwin": true}).to_string()
                } else {
                    format!("hyprwin {} (Hyprland-compatible IPC)\n", env!("CARGO_PKG_VERSION"))
                }
            }
            "debug" => self.q_debug(),
            "configerrors" => self.cfg.warnings.join("\n"),
            "splash" => "Windows, but it gets out of your way.".into(),
            "getoption" | "keyword" | "setcursor" | "output" | "switchxkblayout" | "seterror" | "notify" | "dismissnotify" | "kill" => {
                format!("{cmd}: not supported on Windows yet")
            }
            _ => format!("unknown request '{cmd}'"),
        }
    }

    fn client_json(&self, id: WinId) -> serde_json::Value {
        let w = &self.wm.windows[&id];
        let m = self.meta.get(&id).cloned().unwrap_or_default();
        let r = self.wm.desired_rect(id).or_else(|| dwm::frame_bounds(hwnd(id)).ok()).unwrap_or_default();
        let ws = self.wm.workspaces.get(&w.ws);
        let (ic, it) = self.initial.get(&id).cloned().unwrap_or_default();
        json!({
            "address": format!("0x{id:x}"),
            "mapped": true,
            "hidden": !self.wm.desired_rect(id).is_some(),
            "at": [r.x, r.y],
            "size": [r.w, r.h],
            "workspace": {"id": w.ws, "name": ws.map(|x| x.name.clone()).unwrap_or_default()},
            "floating": w.floating,
            "pseudo": w.pseudo,
            "monitor": ws.map(|x| x.monitor).unwrap_or(0),
            "class": m.class,
            "title": m.title,
            "initialClass": ic,
            "initialTitle": it,
            "pid": m.pid,
            "xwayland": false,
            "pinned": w.pinned,
            "fullscreen": match self.wm.fullscreen_of(id) { Some(FullscreenMode::Fullscreen) => 2, Some(FullscreenMode::Maximized) => 1, None => 0 },
            "focusHistoryID": 0,
            "hyprwin": {"winclass": m.winclass, "exe": m.exe, "minimized": w.minimized},
        })
    }

    fn q_clients(&self, json: bool) -> String {
        let all: Vec<serde_json::Value> = self.wm.windows.keys().map(|&id| self.client_json(id)).collect();
        if json {
            return serde_json::to_string_pretty(&all).unwrap_or_default();
        }
        all.iter().map(client_text).collect()
    }

    fn q_activewindow(&self, json: bool) -> String {
        match self.wm.focus.filter(|f| self.wm.windows.contains_key(f)) {
            Some(f) => {
                let v = self.client_json(f);
                if json { serde_json::to_string_pretty(&v).unwrap_or_default() } else { client_text(&v) }
            }
            None => if json { "{}".into() } else { "Invalid\n".into() },
        }
    }

    fn q_workspaces(&self, json: bool, active_only: bool) -> String {
        let active = self.wm.active_workspace();
        let list: Vec<serde_json::Value> = self
            .wm
            .workspaces
            .values()
            .filter(|w| !active_only || w.id == active)
            .map(|w| {
                let wins: Vec<&hyprwin_layout::wm::Win> = self.wm.windows.values().filter(|x| x.ws == w.id).collect();
                let last = w.last_focus.unwrap_or(0);
                json!({
                    "id": w.id,
                    "name": w.name,
                    "monitor": self.wm.monitors.get(w.monitor).map(|m| m.spec.name.clone()).unwrap_or_default(),
                    "monitorID": w.monitor,
                    "windows": wins.len(),
                    "hasfullscreen": w.fullscreen.is_some(),
                    "lastwindow": format!("0x{last:x}"),
                    "lastwindowtitle": self.meta.get(&last).map(|m| m.title.clone()).unwrap_or_default(),
                    "hyprwin": {"layout": format!("{:?}", w.layout.kind()).to_lowercase()},
                })
            })
            .collect();
        if json {
            return if active_only { serde_json::to_string_pretty(list.first().unwrap_or(&json!({}))).unwrap_or_default() } else { serde_json::to_string_pretty(&list).unwrap_or_default() };
        }
        list.iter()
            .map(|w| {
                format!(
                    "workspace ID {} ({}) on monitor {}:\n\tmonitorID: {}\n\twindows: {}\n\thasfullscreen: {}\n\tlastwindow: {}\n\tlastwindowtitle: {}\n\n",
                    w["id"], w["name"].as_str().unwrap_or(""), w["monitor"].as_str().unwrap_or(""), w["monitorID"], w["windows"], w["hasfullscreen"].as_bool().unwrap_or(false) as u8,
                    w["lastwindow"].as_str().unwrap_or(""), w["lastwindowtitle"].as_str().unwrap_or("")
                )
            })
            .collect()
    }

    fn q_monitors(&self, json: bool) -> String {
        let hw = monitor::monitors();
        let list: Vec<serde_json::Value> = self
            .wm
            .monitors
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let refresh = hw.iter().find(|h| hyprwin_config::model::normalize_monitor(&h.device) == m.spec.name).map(|h| h.refresh_hz).unwrap_or(60);
                let ws = self.wm.workspaces.get(&m.active);
                let sp = m.special.and_then(|s| self.wm.workspaces.get(&s));
                let r = m.spec.rect;
                let wk = m.spec.work;
                json!({
                    "id": i,
                    "name": m.spec.name,
                    "description": m.spec.name,
                    "width": r.w, "height": r.h, "refreshRate": refresh as f64,
                    "x": r.x, "y": r.y,
                    "activeWorkspace": {"id": m.active, "name": ws.map(|w| w.name.clone()).unwrap_or_default()},
                    "specialWorkspace": {"id": m.special.unwrap_or(0), "name": sp.map(|w| w.name.clone()).unwrap_or_default()},
                    "reserved": [wk.x - r.x, wk.y - r.y, r.right() - wk.right(), r.bottom() - wk.bottom()],
                    "scale": m.spec.scale,
                    "focused": i == self.wm.focused_monitor,
                })
            })
            .collect();
        if json {
            return serde_json::to_string_pretty(&list).unwrap_or_default();
        }
        list.iter()
            .map(|m| {
                format!(
                    "Monitor {} (ID {}):\n\t{}x{}@{} at {}x{}\n\tactive workspace: {} ({})\n\tspecial workspace: {} ({})\n\treserved: {}\n\tscale: {}\n\tfocused: {}\n\n",
                    m["name"].as_str().unwrap_or(""), m["id"], m["width"], m["height"], m["refreshRate"], m["x"], m["y"],
                    m["activeWorkspace"]["id"], m["activeWorkspace"]["name"].as_str().unwrap_or(""),
                    m["specialWorkspace"]["id"], m["specialWorkspace"]["name"].as_str().unwrap_or(""),
                    m["reserved"], m["scale"], if m["focused"].as_bool().unwrap_or(false) { "yes" } else { "no" }
                )
            })
            .collect()
    }

    fn q_binds(&self, json: bool) -> String {
        let list: Vec<serde_json::Value> = self
            .binds
            .iter()
            .map(|b| {
                json!({
                    "modmask": b.mods.0, "mods": b.mods.to_string(), "key": format!("{:?}", b.key),
                    "submap": b.submap.clone().unwrap_or_default(), "description": b.description,
                    "dispatcher": format!("{:?}", b.cmd), "repeat": b.flags.repeat, "release": b.flags.release, "mouse": b.flags.mouse,
                })
            })
            .collect();
        if json {
            return serde_json::to_string_pretty(&list).unwrap_or_default();
        }
        list.iter().map(|b| format!("bind: {} {} -> {}\n", b["mods"].as_str().unwrap_or(""), b["key"].as_str().unwrap_or(""), b["dispatcher"].as_str().unwrap_or(""))).collect()
    }

    /// Everything needed to write a rule or file a bug.
    fn q_debug(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("hyprwin {} pid {} uptime {:?}\n", env!("CARGO_PKG_VERSION"), self.own_pid, self.started.elapsed()));
        out.push_str(&format!("config: {} ({} warnings)\n", self.cfg_path.display(), self.cfg.warnings.len()));
        out.push_str(&format!("shell cloak: {}\n", if cloak::self_test().is_ok() { "ok" } else { "UNAVAILABLE (off-screen parking fallback)" }));
        out.push_str(&format!("ledger: {} hidden, {} styled\n", self.ledger.hidden.len(), self.ledger.styled.len()));
        out.push_str(&format!("memory: {:.1} MB working set\n\n", crate::shell::working_set_mb()));
        out.push_str("== monitors / workspaces\n");
        for (i, m) in self.wm.monitors.iter().enumerate() {
            out.push_str(&format!("[{i}] {} rect={:?} work={:?} scale={} active={} special={:?}\n", m.spec.name, m.spec.rect, m.spec.work, m.spec.scale, m.active, m.special));
        }
        for ws in self.wm.workspaces.values() {
            out.push_str(&format!("ws {} '{}' mon={} layout={:?} order={:?} fullscreen={:?}\n", ws.id, ws.name, ws.monitor, ws.layout.kind(), ws.layout_windows(), ws.fullscreen));
        }
        out.push_str("\n== all top-level windows (z-order)\n");
        for h in window::top_level_windows() {
            if !window::is_visible(h) {
                continue;
            }
            let m = classify::read_meta(h);
            let v = classify::classify(h, &m, self.own_pid);
            let managed = self.wm.windows.get(&id_of(h));
            out.push_str(&format!(
                "{:#010x} {} class={} winclass={} title={:?}\n\tstyle={:#010x} ex={:#010x} owner={:#x} pid={} elevated={} cloaked={} verdict={:?}{}\n",
                h.0 as usize,
                if managed.is_some() { "MANAGED" } else { "-------" },
                m.class,
                m.winclass,
                m.title,
                m.style,
                m.ex_style,
                m.owner,
                m.pid,
                m.elevated,
                dwm::cloaked_state(h),
                v,
                managed.map(|w| format!(" ws={} floating={} desired={:?}", w.ws, w.floating, self.wm.desired_rect(w.id))).unwrap_or_default()
            ));
        }
        out
    }
}

fn client_text(v: &serde_json::Value) -> String {
    format!(
        "Window {} -> {}:\n\tmapped: 1\n\tat: {},{}\n\tsize: {},{}\n\tworkspace: {} ({})\n\tfloating: {}\n\tpseudo: {}\n\tmonitor: {}\n\tclass: {}\n\ttitle: {}\n\tinitialClass: {}\n\tinitialTitle: {}\n\tpid: {}\n\txwayland: 0\n\tpinned: {}\n\tfullscreen: {}\n\twinclass: {}\n\n",
        v["address"].as_str().unwrap_or(""),
        v["title"].as_str().unwrap_or(""),
        v["at"][0], v["at"][1], v["size"][0], v["size"][1],
        v["workspace"]["id"], v["workspace"]["name"].as_str().unwrap_or(""),
        v["floating"].as_bool().unwrap_or(false) as u8,
        v["pseudo"].as_bool().unwrap_or(false) as u8,
        v["monitor"],
        v["class"].as_str().unwrap_or(""),
        v["title"].as_str().unwrap_or(""),
        v["initialClass"].as_str().unwrap_or(""),
        v["initialTitle"].as_str().unwrap_or(""),
        v["pid"],
        v["pinned"].as_bool().unwrap_or(false) as u8,
        v["fullscreen"],
        v["hyprwin"]["winclass"].as_str().unwrap_or(""),
    )
}

fn manage_only() -> Option<&'static regex::Regex> {
    static RE: std::sync::OnceLock<Option<regex::Regex>> = std::sync::OnceLock::new();
    RE.get_or_init(|| std::env::var("HYPRWIN_MANAGE_ONLY").ok().and_then(|v| regex::Regex::new(&v).ok())).as_ref()
}

fn near(a: Rect, b: Rect) -> bool {
    (a.x - b.x).abs() <= 1 && (a.y - b.y).abs() <= 1 && (a.w - b.w).abs() <= 1 && (a.h - b.h).abs() <= 1
}

fn regex_lite(v: &str) -> Option<regex::Regex> {
    regex::Regex::new(&format!("(?i){v}")).ok()
}

fn re_or(v: &str) -> Option<regex::Regex> {
    regex_lite(v)
}

pub fn cursor_pos() -> (i32, i32) {
    let mut p = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    (p.x, p.y)
}
