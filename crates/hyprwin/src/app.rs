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
use windows::Win32::Foundation::POINT;
use windows::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::classify::{self, Meta, Verdict};

mod animate;
mod groups;
pub use groups::list as list_groups;
use crate::input;
use crate::{debug, info, warn};

pub const TIMER_VERIFY: usize = 1;
pub const TIMER_MONITORS: usize = 2;
pub const TIMER_SAVE: usize = 3;
pub const TIMER_GROUPS: usize = 4;
pub const TIMER_TASKBAR: usize = 5;
pub const TIMER_CLOSE: usize = 6;

const VERIFY_DELAY_MS: u32 = 150;
const FIGHT_LIMIT: u32 = 4;
const FIGHT_WINDOW: Duration = Duration::from_secs(5);
/// Restores closer together than this are a mass restore ("show desktop").
const RESTORE_BURST: Duration = Duration::from_millis(600);

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
    pending_min: HashMap<WinId, ((i32, i32), Instant)>,
    /// (count, window start) of recent restores, to spot mass restores.
    restores: (u32, Instant),
    stubborn_until: HashMap<WinId, Instant>,
    pending_groups: Vec<groups::PendingGroup>,
    pub paused: bool,
    animator: crate::anim::Animator,
    anim_pending: HashMap<WinId, animate::Pending>,
    /// Last target each window was animated to (re-sending it is a correction).
    anim_target: HashMap<WinId, Rect>,
    anim_active_ws: HashMap<hyprwin_layout::MonId, hyprwin_layout::WsId>,
    anim_special: HashMap<hyprwin_layout::MonId, Option<hyprwin_layout::WsId>>,
    anim_focus: Option<WinId>,
    anim_started: Option<Instant>,
    taskbar_reveal: crate::taskbar::Reveal,
    /// Set while applying corrections that must not animate.
    anim_quiet: bool,
    active_border: crate::border::ActiveBorder,
    /// Windows being closed by hyprwin: (picture window, frame, when).
    closing: HashMap<WinId, (HWND, Rect, Instant)>,
    /// Picture windows whose closing animation is playing.
    close_snaps: Vec<HWND>,
    drag: Option<Drag>,
    last_hover: isize,
    border_override: HashMap<WinId, u32>,
    events: Sender<String>,
    verify_pending: bool,
    pub quit: bool,
    suppress: bool,
    save_pending: bool,
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
            let mut work = crate::taskbar::reclaim(m.rect, m.work);
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
            restores: (0, Instant::now()),
            stubborn_until: HashMap::new(),
            pending_groups: Vec::new(),
            paused: false,
            animator: crate::anim::Animator::new(main),
            anim_pending: HashMap::new(),
            anim_target: HashMap::new(),
            anim_active_ws: HashMap::new(),
            anim_special: HashMap::new(),
            anim_focus: None,
            anim_started: None,
            taskbar_reveal: Default::default(),
            anim_quiet: false,
            active_border: Default::default(),
            closing: HashMap::new(),
            close_snaps: Vec::new(),
            drag: None,
            last_hover: 0,
            border_override: HashMap::new(),
            events,
            verify_pending: false,
            quit: false,
            suppress: false,
            save_pending: false,
            started: Instant::now(),
        };
        app.install_binds();
        let _ = app.ledger.save();
        app
    }

    fn install_binds(&mut self) {
        self.binds = self.cfg.effective_binds();
        if self.paused {
            // While paused only the pause toggle stays bound.
            self.binds.retain(|b| b.cmd == Cmd::TogglePause);
        }
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
        // Add everything first without touching any window, then lay out once
        // (restoring the previous session's layout if there is one), so no
        // window jumps twice.
        self.suppress = true;
        let top_first = window::top_level_windows();
        for &h in top_first.iter().rev() {
            self.try_manage(h);
        }
        // The top of the z-order is what the user used last: treat it as the
        // oldest, so overflow moves the least recently used windows first.
        let order: Vec<WinId> = top_first.iter().map(|&h| id_of(h)).collect();
        self.wm.set_age_order(&order);
        self.suppress = false;
        let fx = match self.load_state() {
            Some(snap) => {
                info!("restoring layout from the previous session");
                self.wm.restore(&snap)
            }
            None => {
                self.wm.invalidate_all();
                self.wm.sync()
            }
        };
        self.apply(fx);
        if let Some(fg) = Some(unsafe { GetForegroundWindow() }).filter(|h| !h.is_invalid()) {
            if self.wm.windows.get(&id_of(fg)).is_some_and(|w| self.wm.desired_rect(w.id).is_some()) {
                let fx = self.wm.on_os_focus(id_of(fg));
                self.apply(fx);
            }
        }
        let ids: Vec<WinId> = self.wm.windows.keys().copied().collect();
        for id in ids {
            self.set_border(id, self.wm.focus == Some(id));
        }
        info!("managing {} window(s) on {} monitor(s)", self.wm.windows.len(), self.wm.monitors.len());
    }

    // ------------------------------------------------------------------ persistence

    fn state_path() -> PathBuf {
        hyprwin_win32::ledger::state_dir().join("state.json")
    }

    /// True while Windows is restoring several windows at once ("show desktop").
    fn restore_burst(&self) -> bool {
        self.restores.0 >= 2 && self.restores.1.elapsed() < RESTORE_BURST * 2
    }

    fn schedule_save(&mut self) {
        if !self.save_pending {
            self.save_pending = true;
            unsafe {
                SetTimer(Some(self.main), TIMER_SAVE, 1000, None);
            }
        }
    }

    /// Writes the layout so a restarted daemon can put everything back.
    pub fn save_state(&mut self) {
        self.save_pending = false;
        unsafe {
            let _ = KillTimer(Some(self.main), TIMER_SAVE);
        }
        let pids: serde_json::Map<String, serde_json::Value> = self.meta.iter().map(|(id, m)| (id.to_string(), json!(m.pid))).collect();
        let v = json!({ "wm": self.wm.snapshot(), "pids": pids });
        let path = Self::state_path();
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, v.to_string()).and_then(|_| std::fs::rename(&tmp, &path)).is_err() {
            warn!("cannot save layout state to {}", path.display());
        }
    }

    /// Loads the saved layout, dropping windows whose handle now belongs to a
    /// different process (handles are reused after a reboot).
    fn load_state(&self) -> Option<serde_json::Value> {
        let mut v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(Self::state_path()).ok()?).ok()?;
        let pids = v["pids"].clone();
        let same_process = |id: u64| pids[id.to_string()].as_u64().map(|p| p as u32) == self.meta.get(&id).map(|m| m.pid);
        if let Some(wins) = v["wm"]["windows"].as_array_mut() {
            wins.retain(|w| w["id"].as_u64().is_some_and(same_process));
        }
        let kept: Vec<u64> = v["wm"]["windows"].as_array()?.iter().filter_map(|w| w["id"].as_u64()).collect();
        if kept.is_empty() {
            return None;
        }
        Some(v["wm"].take())
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
        if classify::quick_reject(h, self.own_pid).is_some() {
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
        // force_tile: only real dialogs (owned windows, #32770) float on their own.
        let is_dialog = matches!(reason, "owned window (dialog/palette)" | "dialog");
        let float = float && (is_dialog || !self.cfg.force_tile);
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
        // An App Group being opened may be waiting for this app.
        let group_slot = self.group_placement(&m.class, reason == "normal app window", &mut nw);
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
        if let Some((gi, slot)) = group_slot {
            self.fill_group_slot(gi, slot, id);
        }
    }

    // ------------------------------------------------------------------ pause

    /// Pausing hands every window back to Windows (shown, borders reset) and
    /// stops managing; resuming re-manages everything and restores the saved
    /// layout. Only the togglepause bind stays active while paused.
    pub fn set_paused(&mut self, paused: bool) {
        if paused == self.paused {
            return;
        }
        if paused {
            self.save_state();
            crate::taskbar::show_all();
            input::shared().watch_edges.store(false, std::sync::atomic::Ordering::Relaxed);
            self.restore_everything();
            self.wm = Wm::new(self.cfg.wm.clone(), monitor_specs(&self.cfg));
            self.meta.clear();
            self.initial.clear();
            self.placed_at.clear();
            self.fights.clear();
            self.stubborn.clear();
            self.stubborn_until.clear();
            self.pending_min.clear();
            self.in_movesize.clear();
            self.pending_groups.clear();
            self.border_override.clear();
            self.drag = None;
            self.paused = true;
            self.sync_border();
            self.install_binds();
            info!("paused: all windows handed back to Windows");
        } else {
            self.paused = false;
            self.install_binds();
            self.manage_existing();
            self.apply_taskbar();
            info!("resumed");
        }
        crate::tray::update(self.main, self.paused);
        self.emit("paused", if self.paused { "1" } else { "0" });
    }

    /// Handles a choice from the tray menu.
    pub fn on_tray(&mut self, cmd: crate::tray::MenuCmd) {
        use crate::tray::MenuCmd;
        match cmd {
            MenuCmd::TogglePause => self.set_paused(!self.paused),
            MenuCmd::Settings => crate::shell::open_settings("settings", None),
            MenuCmd::Themes => crate::shell::open_settings("themes", None),
            MenuCmd::ManageGroups => crate::shell::open_settings("groups", None),
            MenuCmd::SaveGroup => crate::shell::open_settings("groups", Some("--save")),
            MenuCmd::OpenGroup(name) => {
                if let Err(e) = self.open_group(&name) {
                    warn!("{e}");
                }
            }
            MenuCmd::OpenConfig => crate::shell::open_in_notepad(&self.cfg_path),
            MenuCmd::OpenLog => crate::shell::open_in_notepad(&hyprwin_win32::ledger::state_dir().join("hyprwin.log")),
            MenuCmd::Exit => self.quit = true,
        }
    }

    fn unmanage(&mut self, id: WinId) {
        if !self.wm.windows.contains_key(&id) {
            return;
        }
        self.anim_pending.remove(&id);
        self.anim_target.remove(&id);
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
        self.placed_at.remove(&id);
        self.fights.remove(&id);
        self.pending_min.remove(&id);
        self.stubborn.remove(&id);
        self.stubborn_until.remove(&id);
        self.meta.remove(&id);
        self.border_override.remove(&id);
        let fx = self.wm.remove_window(id);
        self.apply(fx);
    }

    // ------------------------------------------------------------------ effects

    pub fn apply(&mut self, fx: Vec<Effect>) {
        if self.suppress {
            return;
        }
        if fx.iter().any(|e| !matches!(e, Effect::Event { .. })) {
            self.schedule_save();
        }
        let mut ledger_dirty = false;
        let plan = self.plan_animations(&fx);
        // Record every window about to be hidden with one ledger write, not
        // one per window (a workspace switch hides them all at once).
        let mut to_hide: HashSet<WinId> = HashSet::new();
        for e in &fx {
            if let Effect::Hide(id) = e {
                let key = hwnd(*id).0 as isize;
                if !self.ledger.hidden.contains_key(&key) {
                    self.ledger.hidden.insert(key, HideMethod::Cloak);
                    to_hide.insert(*id);
                }
            }
        }
        if !to_hide.is_empty() {
            let _ = self.ledger.save();
        }
        for e in fx {
            match e {
                Effect::Show(id) if plan.defer_show.contains(&id) => {}
                Effect::Focus(id) if plan.defer_show.contains(&id) || self.anim_pending.get(&id).is_some_and(|p| p.reveal) => {
                    // Focusing a still-cloaked window would make Windows switch
                    // workspaces; focus it once it is revealed.
                    self.anim_focus = Some(id);
                }
                Effect::Focus(id) => {
                    self.anim_focus = None;
                    self.focus(id);
                }
                Effect::Place { id, rect, anim } => self.place(id, rect, anim),
                Effect::Hide(id) => {
                    // Recorded above, or shown again by an earlier effect in
                    // this batch (record it now); otherwise hidden already.
                    let key = hwnd(id).0 as isize;
                    if to_hide.remove(&id) || !self.ledger.hidden.contains_key(&key) {
                        if self.ledger.hidden.insert(key, HideMethod::Cloak).is_none() {
                            let _ = self.ledger.save();
                        }
                        self.hide(id);
                        ledger_dirty = true;
                    }
                }
                Effect::Show(id) => {
                    self.reveal(id);
                    ledger_dirty = true;
                }
                Effect::Raise(id) => unsafe {
                    let _ = SetWindowPos(hwnd(id), Some(HWND_TOP), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS);
                },
                Effect::Close(id) => self.close_window(id),
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
        self.start_animations(plan);
        self.sync_border();
    }

    /// Puts the drawn border (thick or gradient) around the focused window,
    /// or hides it when there is nothing to draw around.
    pub fn sync_border(&mut self) {
        if self.paused || !self.drawn_borders() {
            self.active_border.hide();
            return;
        }
        let Some(f) = self.wm.focus else {
            self.active_border.hide();
            return;
        };
        let h = hwnd(f);
        let drawable = self.wm.windows.get(&f).is_some_and(|w| !w.minimized)
            && self.wm.desired_rect(f).is_some()
            && self.wm.fullscreen_of(f).is_none()
            && !self.is_animating(f)
            && !self.border_override.contains_key(&f)
            && !self.ledger.hidden.contains_key(&(h.0 as isize))
            && window::is_visible(h)
            && !window::is_minimized(h);
        let frame = if drawable { dwm::frame_bounds(h).ok() } else { None };
        match frame {
            Some(frame) => {
                let scale = crate::border::scale_of(h);
                let st = crate::border::Style {
                    width: ((self.cfg.border_size as f64) * scale).round() as i32,
                    radius: crate::border::corner_radius(self.cfg.rounding, scale),
                    gradient: self.cfg.active_gradient.clone(),
                };
                self.active_border.show(h, frame, st);
            }
            None => self.active_border.hide(),
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
            // Never fight a minimize (e.g. "show desktop" minimizing everything
            // while we are still re-tiling): a minimized window is left alone
            // and the MINIMIZESTART event updates the model.
            if IsIconic(h).as_bool() {
                return;
            }
            if IsZoomed(h).as_bool() {
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

    /// Hides a window that `apply` has already recorded in the ledger as
    /// cloaked (record first, then act: a crash in between still restores it).
    fn hide(&mut self, id: WinId) {
        let h = hwnd(id);
        let key = h.0 as isize;
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
        // Usually there is no floating window on screen at all: skip walking
        // every top-level window on the desktop then.
        let any_float = self.wm.windows.values().any(|f| {
            f.floating && !f.minimized && self.wm.workspaces.get(&f.ws).map(|x| x.monitor) == mon && self.wm.desired_rect(f.id).is_some()
        });
        if !any_float {
            return;
        }
        // Never use an always-on-top window as the anchor: Windows makes a
        // window placed directly below a topmost one topmost too, which once
        // turned tiled apps (Discord, Spotify) into always-on-top windows.
        let is_visible_float = |h: HWND| {
            window::ex_style(h) & WS_EX_TOPMOST.0 == 0
                && self.wm.windows.get(&id_of(h)).is_some_and(|f| {
                    f.floating && !f.minimized && self.wm.desired_rect(f.id).is_some() && self.wm.workspaces.get(&f.ws).map(|x| x.monitor) == mon
                })
        };
        // Top-level windows are enumerated top to bottom: the last float found is the lowest.
        let Some(lowest) = window::top_level_windows().into_iter().filter(|&h| is_visible_float(h)).last() else { return };
        let h = hwnd(focused);
        let was_topmost = window::ex_style(h) & WS_EX_TOPMOST.0 != 0;
        unsafe {
            // Synchronous (only z-order, no resize): we check the result below.
            let _ = SetWindowPos(h, Some(lowest), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            // Belt and braces: we must never leave a window always-on-top.
            if !was_topmost && window::ex_style(h) & WS_EX_TOPMOST.0 != 0 {
                warn!("{:#x} became always-on-top while re-ordering; undoing", focused);
                let _ = SetWindowPos(h, Some(HWND_NOTOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
        }
    }

    fn drawn_borders(&self) -> bool {
        self.cfg.border_size > 0 && crate::border::needed(self.cfg.border_size, &self.cfg.active_gradient)
    }

    fn set_border(&self, id: WinId, active: bool) {
        if self.cfg.border_size <= 0 {
            return;
        }
        if active && self.drawn_borders() && !self.border_override.contains_key(&id) {
            // hyprwin draws this one (see sync_border).
            let _ = dwm::set_border_none(hwnd(id));
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
    /// Re-tiles from here are corrections (refused sizes, learned minimums,
    /// overflow), not user actions, so they never animate.
    pub fn verify(&mut self) {
        self.anim_quiet = true;
        self.verify_inner();
        self.anim_quiet = false;
    }

    fn verify_inner(&mut self) {
        if self.paused {
            self.verify_pending = false;
            return;
        }
        self.verify_pending = false;
        unsafe {
            let _ = KillTimer(Some(self.main), TIMER_VERIFY);
        }
        let ids: Vec<WinId> = self.wm.windows.keys().copied().collect();
        let now = Instant::now();
        // Pauses on stubborn apps expire: try tiling them again.
        let expired: Vec<WinId> = self.stubborn_until.iter().filter(|(_, t)| now >= **t).map(|(id, _)| *id).collect();
        for id in expired {
            self.stubborn_until.remove(&id);
            self.stubborn.remove(&id);
            self.wm.invalidate(id);
        }
        if !self.stubborn_until.is_empty() {
            unsafe {
                SetTimer(Some(self.main), TIMER_VERIFY, 5000, None);
            }
            self.verify_pending = true;
        }
        // Recomputed after anything below re-tiles (it changes other windows' targets).
        let mut desired_all = self.wm.desired_rects();
        for id in ids {
            let h = hwnd(id);
            if !window::is_alive(h) {
                self.unmanage(id);
                desired_all = self.wm.desired_rects();
                continue;
            }
            if self.is_animating(id) {
                self.schedule_verify();
                continue;
            }
            if self.in_movesize.contains(&id) || self.drag.as_ref().is_some_and(|d| d.id == id) || self.stubborn.contains(&id) {
                continue;
            }
            // Minimized (its frame is parked off-screen by Windows): that is not
            // the app moving itself. Make sure the model knows, and move on.
            if unsafe { IsIconic(h).as_bool() } {
                if !self.wm.windows[&id].minimized {
                    let fx = self.wm.set_minimized(id, true);
                    self.apply(fx);
                    desired_all = self.wm.desired_rects();
                }
                continue;
            }
            let Some(desired) = desired_all.get(&id).copied() else { continue };
            if self.ledger.hidden.contains_key(&(h.0 as isize)) {
                continue;
            }
            if unsafe { IsZoomed(h).as_bool() } && !self.wm.windows[&id].floating {
                info!("{:#x} maximized itself: treating as fullscreen 1", id);
                let fx = self.wm.focus_window(id);
                self.apply(fx);
                let fx = self.wm.fullscreen(FullscreenMode::Maximized, Some(true));
                self.apply(fx);
                desired_all = self.wm.desired_rects();
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
                desired_all = self.wm.desired_rects();
                continue;
            }
            let bigger = actual.w > desired.w + 1 || actual.h > desired.h + 1;
            // A real minimum-size refusal: Windows applies the new position but
            // clamps the size. A window that is merely slow to resize is still at
            // its old position, so it must not be mistaken for a minimum.
            let moved_there = (actual.x - desired.x).abs() <= 1 && (actual.y - desired.y).abs() <= 1;
            // A "minimum" that spans (nearly) the whole screen is a window still
            // animating out of a maximized/fullscreen state, not a real minimum
            // (Discord needs ≈ 32 % of the width, Steam ≈ 40 %).
            let work = self.wm.workspaces.get(&self.wm.windows[&id].ws).and_then(|w| self.wm.monitors.get(w.monitor)).map(|m| m.spec.work).unwrap_or(desired);
            let plausible_w = actual.w > desired.w + 1 && (actual.w as f64) < work.w as f64 * 0.9;
            let plausible_h = actual.h > desired.h + 1 && (actual.h as f64) < work.h as f64 * 0.9;
            let refuses = bigger && moved_there && (plausible_w || plausible_h);
            if refuses && !self.pending_min.get(&id).is_some_and(|(s, t)| *s == (actual.w, actual.h) && t.elapsed() >= Duration::from_millis(400)) {
                // First sighting (or too soon to tell): ask again and only learn
                // if it still refuses at least 400 ms later.
                if self.pending_min.get(&id).map(|p| p.0) != Some((actual.w, actual.h)) {
                    self.pending_min.insert(id, ((actual.w, actual.h), now));
                    self.wm.invalidate(id);
                    let fx = self.wm.sync();
                    self.apply(fx);
                    desired_all = self.wm.desired_rects();
                }
                self.schedule_verify();
                continue;
            }
            if refuses {
                self.pending_min.remove(&id);
                // The app refused to shrink twice: learn its minimum and re-tile around it.
                let known = self.wm.windows[&id].min_size;
                let min = (if plausible_w { actual.w } else { known.0 }, if plausible_h { actual.h } else { known.1 });
                if min != known {
                    info!("{:#x} ({}) has a minimum size of {}x{}; re-tiling", id, self.meta.get(&id).map(|m| m.class.as_str()).unwrap_or("?"), min.0, min.1);
                    let fx = self.wm.set_min_size(id, min.0, min.1);
                    self.apply(fx);
                    desired_all = self.wm.desired_rects();
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
                            desired_all = self.wm.desired_rects();
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
                // Keep it tiled; just stop fighting for a while so we never
                // loop with an app that insists on its own geometry.
                warn!("{:#x} ({}) keeps repositioning itself; pausing re-tiling it for 30 s", id, self.meta.get(&id).map(|m| m.class.as_str()).unwrap_or("?"));
                self.stubborn.insert(id);
                self.stubborn_until.insert(id, now + Duration::from_secs(30));
                self.fights.remove(&id);
                continue;
            }
            debug!("{:#x} drifted to {:?}, restoring {:?}", id, actual, desired);
            self.wm.invalidate(id);
            let fx = self.wm.sync();
            self.apply(fx);
            desired_all = self.wm.desired_rects();
        }
    }

    // ------------------------------------------------------------------ events

    pub fn on_winevent(&mut self, ev: u32, h: HWND) {
        if self.paused {
            return;
        }
        let id = id_of(h);
        if matches!(ev, EVENT_OBJECT_HIDE | EVENT_OBJECT_DESTROY | EVENT_OBJECT_CLOAKED) && self.closing.contains_key(&id) {
            self.on_closed(id);
        }
        let managed = self.wm.windows.contains_key(&id);
        let ours = self.ledger.hidden.contains_key(&(h.0 as isize));
        match ev {
            EVENT_OBJECT_SHOW if crate::taskbar::Reveal::is_our_bar_visible(h) => {
                if self.taskbar_reveal.on_shown_by_system() {
                    unsafe {
                        SetTimer(Some(self.main), TIMER_TASKBAR, 120, None);
                    }
                }
            }
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
                debug!("foreground -> {:#x} (managed={managed}, model focus={:?}, animating={})", id, self.wm.focus.map(|f| format!("{f:#x}")), self.anim_pending.len());
                if managed {
                    let hidden_ws = self.wm.windows.get(&id).is_some_and(|w| self.wm.desired_rect(w.id).is_none() && !w.minimized);
                    if hidden_ws && self.restore_burst() {
                        // Windows activated a window it was mass-restoring
                        // ("show desktop") that lives on a hidden workspace:
                        // not a request to switch. Keep the current workspace.
                        debug!("ignoring activation of {:#x} during a mass restore", id);
                        if let Some(f) = self.wm.focus {
                            self.apply(vec![Effect::Focus(f)]);
                        }
                        return;
                    }
                    let fx = self.wm.on_os_focus(id);
                    self.apply(fx);
                    self.keep_floating_on_top(id);
                    self.sync_border();
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
                    // The restore animation moves the window for a moment; that
                    // is not a fight.
                    self.fights.remove(&id);
                    self.placed_at.insert(id, Instant::now());
                    let now = Instant::now();
                    if now.duration_since(self.restores.1) > RESTORE_BURST {
                        self.restores = (0, now);
                    }
                    self.restores.0 += 1;
                    let fx = self.wm.set_minimized(id, false);
                    self.apply(fx);
                    // A single restore of the window now in front is the user
                    // clicking it (taskbar, alt-tab): go to its workspace.
                    let on_hidden_ws = self.wm.desired_rect(id).is_none();
                    if on_hidden_ws && !self.restore_burst() && unsafe { GetForegroundWindow() } == h {
                        let fx = self.wm.on_os_focus(id);
                        self.apply(fx);
                    }
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
                if managed && self.wm.focus == Some(id) {
                    self.sync_border(); // follows the window while it is dragged
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
        if self.paused {
            let _ = input::last_mouse();
            return;
        }
        let (x, y) = input::last_mouse();
        if self.taskbar_reveal.on_mouse(x, y) {
            unsafe {
                SetTimer(Some(self.main), TIMER_TASKBAR, 120, None);
            }
        }
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
        if self.paused {
            return;
        }
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
        // Compiled once per lookup, not once per window.
        let re = match k {
            "class" | "title" | "initialclass" | "initialtitle" => regex_lite(v),
            "address" | "pid" | "floating" | "tiled" => None,
            _ => regex_lite(sel),
        };
        let is = |s: &str| re.as_ref().is_some_and(|r| r.is_match(s));
        self.wm.windows.keys().copied().find(|id| {
            let m = self.meta.get(id);
            match k {
                "address" => u64::from_str_radix(v.trim_start_matches("0x"), 16).ok() == Some(*id),
                "pid" => m.is_some_and(|m| v.parse() == Ok(m.pid)),
                "class" => m.is_some_and(|m| is(&m.class)),
                "title" => m.is_some_and(|m| is(&m.title)),
                "initialclass" => self.initial.get(id).is_some_and(|i| is(&i.0)),
                "initialtitle" => self.initial.get(id).is_some_and(|i| is(&i.1)),
                "floating" => self.wm.windows[id].floating,
                "tiled" => !self.wm.windows[id].floating,
                _ => m.is_some_and(|m| is(&m.class)),
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
            Cmd::MoveWorkspaceToMonitor { ws, monitor } => self.wm.move_workspace_to_monitor(ws.as_ref(), monitor),
            Cmd::SwapActiveWorkspaces(a, b) => self.wm.swap_active_workspaces(a, b),
            Cmd::RenameWorkspace(id, name) => self.wm.rename_workspace(*id, name),
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
            Cmd::TogglePause => {
                self.set_paused(!self.paused);
                return Ok(());
            }
            Cmd::ToggleTaskbar => {
                // Runtime only; the config file decides again on reload.
                self.cfg.hide_taskbar = !self.cfg.hide_taskbar;
                self.apply_taskbar();
                return Ok(());
            }
            Cmd::OpenAppGroup(name) => return self.open_group(name).map(|_| ()),
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
        self.finish_animations_now();
        let cfg = crate::load_config(&self.cfg_path);
        for w in &cfg.warnings {
            warn!("config: {w}");
        }
        info!("config reloaded from {} ({} binds, {} rules, {} warnings)", self.cfg_path.display(), cfg.binds.len(), cfg.window_rules.len(), cfg.warnings.len());
        crate::watch_config(cfg.files.clone());
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
        // `addreserved` may have changed; apply_taskbar refreshes only when
        // the taskbar itself changes.
        let taskbar_changes = (self.cfg.hide_taskbar && !self.paused) != crate::taskbar::is_hidden();
        self.apply_taskbar();
        if !taskbar_changes {
            self.refresh_monitors();
        }
    }

    // ------------------------------------------------------------------ taskbar

    /// Hides or shows the Windows taskbar to match `hyprwin:hide_taskbar`.
    pub fn apply_taskbar(&mut self) {
        let want = self.cfg.hide_taskbar && !self.paused;
        input::shared().watch_edges.store(want, std::sync::atomic::Ordering::Relaxed);
        if want == crate::taskbar::is_hidden() {
            return;
        }
        if want {
            let ledger = &mut self.ledger;
            let n = crate::taskbar::hide_all(&mut |hs: &[isize]| {
                for h in hs {
                    if !ledger.taskbars.contains(h) {
                        ledger.taskbars.push(*h);
                    }
                }
                let _ = ledger.save();
            });
            info!("taskbar hidden ({n} bar(s)); touch its screen edge to reveal it");
        } else {
            let shown = crate::taskbar::show_all();
            self.ledger.taskbars.retain(|h| !shown.contains(h));
            let _ = self.ledger.save();
            info!("taskbar shown");
        }
        self.refresh_monitors();
    }

    /// Explorer restarted: its taskbars are new windows.
    pub fn on_explorer_restart(&mut self) {
        let old = crate::taskbar::show_all();
        self.ledger.taskbars.retain(|h| !old.contains(h));
        let _ = self.ledger.save();
        self.apply_taskbar();
    }

    pub fn taskbar_tick(&mut self) {
        if !self.taskbar_reveal.tick() {
            unsafe {
                let _ = KillTimer(Some(self.main), TIMER_TASKBAR);
            }
        }
    }

    pub fn refresh_monitors(&mut self) {
        crate::taskbar::monitors_changed();
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
        self.animator.cancel();
        for (_, (snap, _, _)) in std::mem::take(&mut self.closing) {
            crate::snapshot::destroy(snap);
        }
        for snap in std::mem::take(&mut self.close_snaps) {
            crate::snapshot::destroy(snap);
        }
        self.anim_pending.clear();
        self.anim_target.clear();
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
            // hyprwin extensions (used by the Settings window and scripts)
            "status" => json!({ "paused": self.paused, "windows": self.wm.windows.len(), "version": env!("CARGO_PKG_VERSION") }).to_string(),
            "pause" => {
                self.set_paused(true);
                "ok".into()
            }
            "resume" => {
                self.set_paused(false);
                "ok".into()
            }
            "groups" => {
                let v: Vec<serde_json::Value> = list_groups().into_iter().map(|(n, c)| json!({ "name": n, "apps": c })).collect();
                serde_json::to_string(&v).unwrap_or_default()
            }
            "groupsave" => self.save_group(rest).unwrap_or_else(|e| format!("error: {e}")),
            "groupopen" => self.open_group(rest).unwrap_or_else(|e| format!("error: {e}")),
            "groupdelete" => match groups::delete(rest) {
                Ok(()) => "ok".into(),
                Err(e) => format!("error: {e}"),
            },
            "settings" => {
                crate::shell::open_settings(if rest.is_empty() { "settings" } else { rest }, None);
                "ok".into()
            }
            "getoption" | "keyword" | "setcursor" | "output" | "switchxkblayout" | "seterror" | "notify" | "dismissnotify" | "kill" => {
                format!("{cmd}: not supported on Windows yet")
            }
            _ => format!("unknown request '{cmd}'"),
        }
    }

    fn client_json(&self, id: WinId) -> serde_json::Value {
        let w = &self.wm.windows[&id];
        let m = self.meta.get(&id).cloned().unwrap_or_default();
        let r = self.wm.layout_rect(id).or_else(|| dwm::frame_bounds(hwnd(id)).ok()).unwrap_or_default();
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
                    // Same fields as `hyprctl binds -j`, plus a plain-English "action".
                    "modmask": b.mods.0, "mods": b.mods.to_string(), "key": b.key.name(),
                    "submap": b.submap.clone().unwrap_or_default(), "description": b.description, "has_description": !b.description.is_empty(),
                    "dispatcher": b.dispatcher, "arg": b.arg, "action": b.cmd.describe(),
                    "locked": b.flags.locked, "repeat": b.flags.repeat, "release": b.flags.release, "mouse": b.flags.mouse, "non_consuming": b.flags.non_consuming,
                })
            })
            .collect();
        if json {
            return serde_json::to_string_pretty(&list).unwrap_or_default();
        }
        list.iter().map(|b| format!("bind: {} {} -> {} {}\n", b["mods"].as_str().unwrap_or(""), b["key"].as_str().unwrap_or(""), b["dispatcher"].as_str().unwrap_or(""), b["arg"].as_str().unwrap_or(""))).collect()
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

pub fn cursor_pos() -> (i32, i32) {
    let mut p = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    (p.x, p.y)
}
