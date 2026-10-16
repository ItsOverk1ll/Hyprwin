//! The window-manager state machine.
//!
//! Every public operation mutates the model and then calls [`Wm::sync`], which
//! diffs the desired world (rects, visibility, focus) against what was last
//! emitted and returns only the [`Effect`]s needed to get there.

use std::collections::BTreeMap;

use crate::dwindle::{Dwindle, DwindleConfig};
use crate::geom::{Dir, Gaps, Rect, apply_inner_gaps, contains_point, neighbor, shrink};
use crate::master::{Master, MasterConfig};
use crate::{MonId, WinId, WsId};

// ------------------------------------------------------------------ config

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutKind {
    Dwindle,
    Master,
}

#[derive(Clone, Debug)]
pub struct WorkspaceRule {
    /// Workspace id, or name for `name:` workspaces.
    pub id: Option<WsId>,
    pub name: Option<String>,
    pub monitor: Option<String>,
    pub default: bool,
    pub persistent: bool,
    pub gaps_in: Option<Gaps>,
    pub gaps_out: Option<Gaps>,
    pub layout: Option<LayoutKind>,
}

#[derive(Clone, Debug)]
pub struct WmConfig {
    pub gaps_in: Gaps,
    pub gaps_out: Gaps,
    /// Space reserved around each window for a drawn border (0 when DWM draws it).
    pub border_reserve: i32,
    pub layout: LayoutKind,
    pub dwindle: DwindleConfig,
    pub master: MasterConfig,
    pub workspace_back_and_forth: bool,
    /// 0 = ignore, 1 = take over (focused window goes fullscreen), 2 = unfullscreen.
    pub on_focus_under_fullscreen: u8,
    pub workspace_rules: Vec<WorkspaceRule>,
}

impl Default for WmConfig {
    fn default() -> Self {
        // Hyprland defaults.
        Self {
            gaps_in: Gaps::all(5),
            gaps_out: Gaps::all(20),
            border_reserve: 0,
            layout: LayoutKind::Dwindle,
            dwindle: DwindleConfig::default(),
            master: MasterConfig::default(),
            workspace_back_and_forth: false,
            on_focus_under_fullscreen: 2,
            workspace_rules: Vec::new(),
        }
    }
}

// ------------------------------------------------------------------ model

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FullscreenMode {
    /// Hyprland mode 0: whole monitor.
    Fullscreen,
    /// Hyprland mode 1: whole work area, gaps kept.
    Maximized,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anim {
    None,
    Open,
    Move,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Put the window's visible frame at `rect`.
    Place { id: WinId, rect: Rect, anim: Anim },
    Show(WinId),
    Hide(WinId),
    Focus(WinId),
    Raise(WinId),
    Close(WinId),
    Kill(WinId),
    SetFullscreen { id: WinId, on: bool },
    Border { id: WinId, active: bool },
    WarpCursor { x: i32, y: i32 },
    /// Hyprland socket2 event (`name>>data`).
    Event { name: &'static str, data: String },
}

#[derive(Clone, Debug)]
pub struct Win {
    pub id: WinId,
    pub ws: WsId,
    pub floating: bool,
    pub float_rect: Rect,
    pub pinned: bool,
    pub pseudo: bool,
    pub minimized: bool,
    /// Preferred size for pseudotiling (the window's own size when it opened).
    pub natural: (i32, i32),
    /// Smallest visible frame the app accepts (learned at runtime).
    pub min_size: (i32, i32),
    /// Floated by the WM because it did not fit (not by the user); it goes
    /// back to tiling as soon as there is room.
    pub auto_float: bool,
    /// Order in which windows were added (newest = largest).
    pub seq: u64,
    // Last emitted state, for diffing.
    placed: Option<Rect>,
    shown: bool,
}

#[derive(Clone, Debug)]
pub enum Layout {
    Dwindle(Dwindle),
    Master(Master),
}

impl Layout {
    fn new(k: LayoutKind) -> Self {
        match k {
            LayoutKind::Dwindle => Layout::Dwindle(Dwindle::default()),
            LayoutKind::Master => Layout::Master(Master::default()),
        }
    }
    fn windows(&self) -> Vec<WinId> {
        match self {
            Layout::Dwindle(d) => d.windows(),
            Layout::Master(m) => m.windows(),
        }
    }
    fn remove(&mut self, w: WinId) {
        match self {
            Layout::Dwindle(d) => {
                d.remove(w);
            }
            Layout::Master(m) => {
                m.remove(w);
            }
        }
    }
    fn contains(&self, w: WinId) -> bool {
        match self {
            Layout::Dwindle(d) => d.contains(w),
            Layout::Master(m) => m.contains(w),
        }
    }
    pub fn kind(&self) -> LayoutKind {
        match self {
            Layout::Dwindle(_) => LayoutKind::Dwindle,
            Layout::Master(_) => LayoutKind::Master,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Ws {
    pub id: WsId,
    pub name: String,
    pub monitor: MonId,
    pub layout: Layout,
    pub last_focus: Option<WinId>,
    pub fullscreen: Option<(WinId, FullscreenMode)>,
    pub persistent: bool,
    pub gaps_in: Option<Gaps>,
    pub gaps_out: Option<Gaps>,
}

impl Ws {
    pub fn is_special(&self) -> bool {
        self.id < 0
    }
    /// Tiled windows in layout order.
    pub fn layout_windows(&self) -> Vec<WinId> {
        self.layout.windows()
    }
}

#[derive(Clone, Debug)]
pub struct MonitorSpec {
    pub name: String,
    pub rect: Rect,
    /// Work area (monitor minus reserved space such as a bar).
    pub work: Rect,
    pub scale: f64,
}

#[derive(Clone, Debug)]
pub struct Mon {
    pub spec: MonitorSpec,
    pub active: WsId,
    pub special: Option<WsId>,
    pub previous: Option<WsId>,
}

/// Parameters for a newly managed window.
#[derive(Clone, Debug, Default)]
pub struct NewWindow {
    /// Current visible frame.
    pub rect: Rect,
    pub floating: bool,
    pub monitor: Option<MonId>,
    pub workspace: Option<WsSel>,
    pub fullscreen: bool,
    pub pseudo: bool,
    pub no_initial_focus: bool,
    pub cursor: Option<(f64, f64)>,
    /// Window-rule size/position for floating windows.
    pub float_rect: Option<Rect>,
    pub center: bool,
    pub pin: bool,
}

// ------------------------------------------------------------------ workspace selectors

#[derive(Clone, Debug, PartialEq)]
pub enum WsSel {
    Id(WsId),
    Name(String),
    Relative(i64),
    MonitorRelative(i64),
    OpenRelative(i64),
    Previous,
    Empty,
    Special(Option<String>),
}

impl WsSel {
    pub fn parse(s: &str) -> Option<WsSel> {
        let s = s.trim();
        let rel = |t: &str| t.parse::<i64>().ok();
        Some(match s {
            "previous" | "previous_per_monitor" => WsSel::Previous,
            "empty" | "emptynm" | "emptym" => WsSel::Empty,
            "special" => WsSel::Special(None),
            _ if s.starts_with("special:") => WsSel::Special(Some(s["special:".len()..].to_string())),
            _ if s.starts_with("name:") => WsSel::Name(s["name:".len()..].to_string()),
            _ if s.starts_with('m') && s.len() > 1 => WsSel::MonitorRelative(rel(&s[1..])?),
            _ if s.starts_with('e') && s.len() > 1 => WsSel::OpenRelative(rel(&s[1..])?),
            _ if s.starts_with('r') && s.len() > 1 => WsSel::Relative(rel(&s[1..])?),
            _ if s.starts_with('+') || s.starts_with('-') => WsSel::Relative(rel(s)?),
            _ => match s.parse::<i64>() {
                Ok(n) if n >= 1 => WsSel::Id(n),
                _ => WsSel::Name(s.to_string()),
            },
        })
    }
}

// ------------------------------------------------------------------ Wm

pub struct Wm {
    pub cfg: WmConfig,
    pub windows: BTreeMap<WinId, Win>,
    pub workspaces: BTreeMap<WsId, Ws>,
    pub monitors: Vec<Mon>,
    pub focus: Option<WinId>,
    pub focused_monitor: MonId,
    history: Vec<WinId>,
    effects: Vec<Effect>,
    emitted_focus: Option<WinId>,
    next_named: WsId,
    next_special: WsId,
}

impl Wm {
    pub fn new(cfg: WmConfig, monitors: Vec<MonitorSpec>) -> Wm {
        let mut wm = Wm {
            cfg,
            windows: BTreeMap::new(),
            workspaces: BTreeMap::new(),
            monitors: Vec::new(),
            focus: None,
            focused_monitor: 0,
            history: Vec::new(),
            effects: Vec::new(),
            emitted_focus: None,
            next_named: 10_000,
            next_special: -98,
        };
        wm.set_monitors(monitors);
        wm.effects.clear();
        wm
    }

    // -------------------------------------------------------------- monitors

    /// Replaces the monitor set (hotplug, resolution or DPI change). Workspaces
    /// on removed monitors move to the first remaining monitor.
    pub fn set_monitors(&mut self, specs: Vec<MonitorSpec>) -> Vec<Effect> {
        let old: Vec<Mon> = std::mem::take(&mut self.monitors);
        let mut used: Vec<WsId> = Vec::new();
        for spec in specs {
            let prev = old.iter().find(|m| m.spec.name == spec.name);
            let active = prev.map(|m| m.active).filter(|id| self.workspaces.contains_key(id));
            self.monitors.push(Mon {
                active: active.unwrap_or(0),
                special: prev.and_then(|m| m.special),
                previous: prev.and_then(|m| m.previous),
                spec,
            });
        }
        if self.monitors.is_empty() {
            return Vec::new();
        }
        // Re-home workspaces whose monitor index no longer exists or changed.
        let names: Vec<String> = self.monitors.iter().map(|m| m.spec.name.clone()).collect();
        for ws in self.workspaces.values_mut() {
            let old_name = old.get(ws.monitor).map(|m| m.spec.name.clone());
            ws.monitor = old_name.and_then(|n| names.iter().position(|x| *x == n)).unwrap_or(0);
        }
        for i in 0..self.monitors.len() {
            if self.monitors[i].active != 0 {
                used.push(self.monitors[i].active);
                continue;
            }
            // Pick: rule default for this monitor, else the lowest free id.
            let name = self.monitors[i].spec.name.clone();
            let from_rule = self
                .cfg
                .workspace_rules
                .iter()
                .find(|r| r.default && r.monitor.as_deref() == Some(name.as_str()))
                .and_then(|r| r.id);
            let id = from_rule.filter(|id| !used.contains(id)).unwrap_or_else(|| {
                let mut n = 1;
                while used.contains(&n) || self.workspaces.get(&n).is_some_and(|w| w.monitor != i && self.is_active_anywhere(n)) {
                    n += 1;
                }
                n
            });
            self.ensure_ws(id, None, i);
            if let Some(ws) = self.workspaces.get_mut(&id) {
                ws.monitor = i;
            }
            self.monitors[i].active = id;
            used.push(id);
        }
        self.focused_monitor = self.focused_monitor.min(self.monitors.len() - 1);
        self.sync()
    }

    fn is_active_anywhere(&self, id: WsId) -> bool {
        self.monitors.iter().any(|m| m.active == id || m.special == Some(id))
    }

    pub fn monitor_at(&self, x: f64, y: f64) -> Option<MonId> {
        self.monitors.iter().position(|m| contains_point(m.spec.rect, x, y))
    }

    fn monitor_for_rect(&self, r: Rect) -> MonId {
        let (cx, cy) = r.center();
        self.monitor_at(cx, cy).unwrap_or(self.focused_monitor)
    }

    // -------------------------------------------------------------- workspaces

    fn ensure_ws(&mut self, id: WsId, name: Option<&str>, monitor: MonId) -> WsId {
        if !self.workspaces.contains_key(&id) {
            let rule = self.cfg.workspace_rules.iter().find(|r| r.id == Some(id) || (name.is_some() && r.name.as_deref() == name)).cloned();
            let monitor = rule
                .as_ref()
                .and_then(|r| r.monitor.as_ref())
                .and_then(|n| self.monitors.iter().position(|m| &m.spec.name == n))
                .unwrap_or(monitor);
            let layout = rule.as_ref().and_then(|r| r.layout).unwrap_or(self.cfg.layout);
            let ws = Ws {
                id,
                name: name.map(str::to_string).unwrap_or_else(|| id.to_string()),
                monitor,
                layout: Layout::new(layout),
                last_focus: None,
                fullscreen: None,
                persistent: rule.as_ref().is_some_and(|r| r.persistent),
                gaps_in: rule.as_ref().and_then(|r| r.gaps_in),
                gaps_out: rule.as_ref().and_then(|r| r.gaps_out),
            };
            self.workspaces.insert(id, ws);
            let kind = if id < 0 { "createworkspace" } else { "createworkspace" };
            self.effects.push(Effect::Event { name: kind, data: self.workspaces[&id].name.clone() });
        }
        id
    }

    fn ws_windows(&self, ws: WsId) -> impl Iterator<Item = &Win> {
        self.windows.values().filter(move |w| w.ws == ws)
    }

    /// Destroys empty, inactive, non-persistent workspaces (Hyprland behaviour).
    fn gc_workspaces(&mut self) {
        let dead: Vec<WsId> = self
            .workspaces
            .values()
            .filter(|ws| !ws.persistent && !self.is_active_anywhere(ws.id) && self.ws_windows(ws.id).next().is_none())
            .map(|ws| ws.id)
            .collect();
        for id in dead {
            if let Some(ws) = self.workspaces.remove(&id) {
                self.effects.push(Effect::Event { name: "destroyworkspace", data: ws.name });
            }
        }
    }

    fn named_ws(&mut self, name: &str, special: bool) -> WsId {
        let full = if special { format!("special:{name}") } else { name.to_string() };
        if let Some(ws) = self.workspaces.values().find(|w| w.name == full) {
            return ws.id;
        }
        if let Some(r) = self.cfg.workspace_rules.iter().find(|r| r.name.as_deref() == Some(full.as_str())).and_then(|r| r.id) {
            return r;
        }
        if special {
            self.next_special -= 1;
            self.next_special + 1
        } else {
            self.next_named += 1;
            self.next_named - 1
        }
    }

    /// Resolves a selector to (workspace id, display name), creating nothing.
    pub fn resolve(&mut self, sel: &WsSel) -> Option<(WsId, String)> {
        let mon = self.focused_monitor;
        let cur = self.monitors.get(mon)?.active;
        let on_mon: Vec<WsId> = self.workspaces.values().filter(|w| w.monitor == mon && w.id > 0).map(|w| w.id).collect();
        let open: Vec<WsId> = self.workspaces.values().filter(|w| w.id > 0 && (self.ws_windows(w.id).next().is_some() || self.is_active_anywhere(w.id))).map(|w| w.id).collect();
        let cycle = |list: &[WsId], d: i64| -> Option<WsId> {
            let i = list.iter().position(|&x| x == cur)? as i64;
            let n = list.len() as i64;
            Some(list[((i + d).rem_euclid(n)) as usize])
        };
        let id = match sel {
            WsSel::Id(n) => *n,
            WsSel::Name(n) => self.named_ws(n, false),
            WsSel::Relative(d) => (cur + d).max(1),
            WsSel::MonitorRelative(d) => cycle(&on_mon, *d)?,
            WsSel::OpenRelative(d) => cycle(&open, *d)?,
            WsSel::Previous => self.monitors[mon].previous?,
            WsSel::Empty => (1..).find(|n| self.ws_windows(*n).next().is_none() && !self.is_active_anywhere(*n)).unwrap(),
            WsSel::Special(n) => {
                let name = n.clone().unwrap_or_else(|| "special".into());
                let id = self.named_ws(&name, true);
                return Some((id, format!("special:{name}")));
            }
        };
        let name = match sel {
            WsSel::Name(n) => n.clone(),
            _ => self.workspaces.get(&id).map(|w| w.name.clone()).unwrap_or_else(|| id.to_string()),
        };
        Some((id, name))
    }

    /// `workspace <sel>`.
    pub fn switch_workspace(&mut self, sel: &WsSel) -> Vec<Effect> {
        if let WsSel::Special(_) = sel {
            return self.toggle_special(sel);
        }
        let Some((mut id, name)) = self.resolve(sel) else { return self.sync() };
        let mon = self.focused_monitor;
        if id == self.monitors[mon].active {
            if !self.cfg.workspace_back_and_forth {
                return self.sync();
            }
            match self.monitors[mon].previous {
                Some(p) => id = p,
                None => return self.sync(),
            }
        }
        self.ensure_ws(id, Some(&name), mon);
        let target_mon = self.workspaces[&id].monitor;
        if self.monitors.get(target_mon).is_some_and(|m| m.active == id) {
            // Already visible elsewhere: just focus that monitor.
            self.focus_monitor_idx(target_mon);
        } else {
            let target_mon = if self.is_active_anywhere(id) { target_mon } else { mon };
            let prev = self.monitors[target_mon].active;
            self.workspaces.get_mut(&id).unwrap().monitor = target_mon;
            self.monitors[target_mon].previous = Some(prev);
            self.monitors[target_mon].active = id;
            self.move_pinned(prev, id);
            self.focused_monitor = target_mon;
            self.effects.push(Effect::Event { name: "workspace", data: self.workspaces[&id].name.clone() });
            self.effects.push(Effect::Event { name: "workspacev2", data: format!("{id},{}", self.workspaces[&id].name) });
        }
        self.focus = self.workspaces[&id].last_focus.filter(|w| self.windows.contains_key(w)).or_else(|| self.first_window_of(id));
        self.gc_workspaces();
        self.sync()
    }

    fn move_pinned(&mut self, from: WsId, to: WsId) {
        let pinned: Vec<WinId> = self.windows.values().filter(|w| w.ws == from && w.pinned).map(|w| w.id).collect();
        for id in pinned {
            self.windows.get_mut(&id).unwrap().ws = to;
        }
    }

    fn first_window_of(&self, ws: WsId) -> Option<WinId> {
        self.history.iter().rev().copied().find(|w| self.windows.get(w).is_some_and(|x| x.ws == ws && !x.minimized)).or_else(|| {
            self.workspaces.get(&ws).and_then(|w| w.layout.windows().first().copied()).or_else(|| self.ws_windows(ws).find(|w| !w.minimized).map(|w| w.id))
        })
    }

    /// `togglespecialworkspace [name]`.
    pub fn toggle_special(&mut self, sel: &WsSel) -> Vec<Effect> {
        let Some((id, name)) = self.resolve(sel) else { return self.sync() };
        let mon = self.focused_monitor;
        if self.monitors[mon].special == Some(id) {
            self.monitors[mon].special = None;
            let active = self.monitors[mon].active;
            self.focus = self.workspaces.get(&active).and_then(|w| w.last_focus).or_else(|| self.first_window_of(active));
            self.effects.push(Effect::Event { name: "activespecial", data: format!(",{}", self.monitors[mon].spec.name) });
        } else {
            // A special workspace can only be open on one monitor.
            for m in &mut self.monitors {
                if m.special == Some(id) {
                    m.special = None;
                }
            }
            self.ensure_ws(id, Some(&name), mon);
            self.workspaces.get_mut(&id).unwrap().monitor = mon;
            self.monitors[mon].special = Some(id);
            self.focus = self.workspaces[&id].last_focus.or_else(|| self.first_window_of(id));
            self.effects.push(Effect::Event { name: "activespecial", data: format!("{name},{}", self.monitors[mon].spec.name) });
        }
        self.gc_workspaces();
        self.sync()
    }

    /// `movetoworkspace` / `movetoworkspacesilent`.
    pub fn move_to_workspace(&mut self, win: Option<WinId>, sel: &WsSel, follow: bool) -> Vec<Effect> {
        let Some(w) = win.or(self.focus) else { return self.sync() };
        let Some((id, name)) = self.resolve(sel) else { return self.sync() };
        let Some(src) = self.windows.get(&w).map(|x| x.ws) else { return self.sync() };
        if src == id {
            return self.sync();
        }
        let mon = self.workspaces.get(&src).map(|x| x.monitor).unwrap_or(self.focused_monitor);
        self.ensure_ws(id, Some(&name), mon);
        self.detach(w);
        {
            let win = self.windows.get_mut(&w).unwrap();
            win.ws = id;
            win.pinned = false;
            // A window floated only because it did not fit gets another chance.
            if win.auto_float {
                win.floating = false;
            }
        }
        self.attach(w, None);
        if !self.windows[&w].floating {
            if self.ws_fits(id) {
                self.windows.get_mut(&w).unwrap().auto_float = false;
            } else {
                // Would squeeze something below its minimum: float until there is room.
                self.detach(w);
                let win = self.windows.get_mut(&w).unwrap();
                win.floating = true;
                win.auto_float = true;
            }
        }
        self.retry_auto_floats(src);
        self.effects.push(Effect::Event { name: "movewindow", data: format!("{w:x},{name}") });
        self.effects.push(Effect::Event { name: "movewindowv2", data: format!("{w:x},{id},{name}") });
        if follow {
            if id < 0 {
                if self.monitors[self.focused_monitor].special != Some(id) {
                    let fx = self.toggle_special(sel);
                    self.effects.extend(fx);
                }
            } else {
                let fx = self.switch_workspace(&WsSel::Id(id));
                self.effects.extend(fx);
            }
            self.focus = Some(w);
        } else if self.focus == Some(w) {
            self.focus = self.first_window_of(src);
        }
        self.gc_workspaces();
        self.sync()
    }

    // -------------------------------------------------------------- windows

    pub fn add_window(&mut self, id: WinId, nw: NewWindow) -> Vec<Effect> {
        if self.windows.contains_key(&id) {
            return self.sync();
        }
        let mon = nw.monitor.unwrap_or_else(|| self.monitor_for_rect(nw.rect)).min(self.monitors.len().saturating_sub(1));
        let ws = match &nw.workspace {
            Some(sel) => {
                let (wid, name) = self.resolve(sel).unwrap_or((self.monitors[mon].active, String::new()));
                self.ensure_ws(wid, Some(&name), mon)
            }
            None => self.monitors[mon].special.unwrap_or(self.monitors[mon].active),
        };
        let mut float_rect = nw.float_rect.unwrap_or(nw.rect);
        let ws_mon = self.workspaces[&ws].monitor;
        if nw.center || !rect_inside(float_rect, self.monitors[ws_mon].spec.rect) {
            float_rect = center_in(float_rect, self.monitors[ws_mon].spec.work);
        }
        self.windows.insert(
            id,
            Win {
                id,
                ws,
                floating: nw.floating,
                float_rect,
                pinned: nw.pin && nw.floating,
                pseudo: nw.pseudo,
                minimized: false,
                natural: (nw.rect.w, nw.rect.h),
                min_size: (0, 0),
                auto_float: false,
                seq: self.windows.values().map(|w| w.seq).max().unwrap_or(0) + 1,
                placed: None,
                shown: true,
            },
        );
        // Opening a tiled window on a workspace with a fullscreen window un-fullscreens it.
        if !nw.floating && self.cfg.on_focus_under_fullscreen == 2 {
            if let Some(ws) = self.workspaces.get_mut(&ws) {
                if let Some((fw, _)) = ws.fullscreen.take() {
                    self.effects.push(Effect::SetFullscreen { id: fw, on: false });
                }
            }
        }
        self.attach(id, nw.cursor);
        if nw.fullscreen {
            self.workspaces.get_mut(&ws).unwrap().fullscreen = Some((id, FullscreenMode::Fullscreen));
            self.effects.push(Effect::SetFullscreen { id, on: true });
        }
        self.effects.push(Effect::Event { name: "openwindow", data: format!("{id:x},{}", self.workspaces[&ws].name) });
        if !nw.no_initial_focus && self.is_visible_ws(ws) {
            self.set_focus(Some(id));
        }
        let mut fx = self.sync();
        for e in fx.iter_mut() {
            if let Effect::Place { id: pid, anim, .. } = e {
                if *pid == id {
                    *anim = Anim::Open;
                }
            }
        }
        fx
    }

    pub fn remove_window(&mut self, id: WinId) -> Vec<Effect> {
        let Some(w) = self.windows.get(&id).cloned() else { return Vec::new() };
        self.detach(id);
        self.windows.remove(&id);
        self.history.retain(|&x| x != id);
        if let Some(ws) = self.workspaces.get_mut(&w.ws) {
            if ws.last_focus == Some(id) {
                ws.last_focus = None;
            }
            if ws.fullscreen.is_some_and(|f| f.0 == id) {
                ws.fullscreen = None;
            }
        }
        self.effects.push(Effect::Event { name: "closewindow", data: format!("{id:x}") });
        if self.focus == Some(id) {
            self.focus = None;
            self.emitted_focus = None;
            let next = self.first_window_of(w.ws);
            self.set_focus(next);
        }
        self.retry_auto_floats(w.ws);
        self.gc_workspaces();
        self.sync()
    }

    /// Inserts a window into its workspace layout (if tiled and not minimized).
    fn attach(&mut self, id: WinId, cursor: Option<(f64, f64)>) {
        let Some(w) = self.windows.get(&id) else { return };
        if w.floating || w.minimized {
            return;
        }
        let ws_id = w.ws;
        let area = self.layout_area(ws_id);
        let focused_target = self.focus.filter(|f| self.windows.get(f).is_some_and(|x| x.ws == ws_id && !x.floating));
        let cursor_target = cursor.and_then(|(x, y)| self.tiled_window_at(ws_id, x, y));
        let target = if self.cfg.dwindle.use_active_for_splits { focused_target.or(cursor_target) } else { cursor_target.or(focused_target) };
        let dcfg = self.cfg.dwindle.clone();
        let mcfg = self.cfg.master.clone();
        let ws = self.workspaces.get_mut(&ws_id).unwrap();
        match &mut ws.layout {
            Layout::Dwindle(d) => d.insert(id, target, cursor, area, &dcfg),
            Layout::Master(m) => m.insert(id, target, &mcfg),
        }
    }

    fn detach(&mut self, id: WinId) {
        let Some(ws) = self.windows.get(&id).map(|w| w.ws) else { return };
        if let Some(ws) = self.workspaces.get_mut(&ws) {
            ws.layout.remove(id);
        }
    }

    fn tiled_window_at(&self, ws: WsId, x: f64, y: f64) -> Option<WinId> {
        self.tiled_rects(ws).into_iter().find(|(_, r)| contains_point(*r, x, y)).map(|(w, _)| w)
    }

    pub fn set_minimized(&mut self, id: WinId, minimized: bool) -> Vec<Effect> {
        let Some(w) = self.windows.get(&id) else { return Vec::new() };
        if w.minimized == minimized {
            return self.sync();
        }
        if minimized {
            self.detach(id);
            let w = self.windows.get_mut(&id).unwrap();
            w.minimized = true;
            w.placed = None;
            // The daemon uncloaks minimized windows immediately (a window that
            // is both cloaked and minimized can vanish for good), so from the
            // model's point of view it is shown again.
            w.shown = true;
            if self.focus == Some(id) {
                let ws = self.windows[&id].ws;
                self.focus = None;
                let next = self.first_window_of(ws);
                self.set_focus(next);
            }
            let ws = self.windows[&id].ws;
            self.retry_auto_floats(ws);
        } else {
            self.windows.get_mut(&id).unwrap().minimized = false;
            self.attach(id, None);
            // Restoring a window on a hidden workspace brings that workspace up.
            let ws = self.windows[&id].ws;
            if !self.is_visible_ws(ws) && ws > 0 {
                self.focused_monitor = self.workspaces[&ws].monitor;
                let fx = self.switch_workspace(&WsSel::Id(ws));
                self.effects.extend(fx);
            }
            self.set_focus(Some(id));
        }
        self.effects.push(Effect::Event { name: "minimized", data: format!("{id:x},{}", minimized as u8) });
        self.sync()
    }

    /// The OS (user click, alt-tab, app activation) focused a window.
    pub fn on_os_focus(&mut self, id: WinId) -> Vec<Effect> {
        let Some(w) = self.windows.get(&id) else { return self.sync() };
        if w.minimized {
            return self.sync();
        }
        let ws = w.ws;
        if !self.is_visible_ws(ws) {
            if ws < 0 {
                let name = self.workspaces[&ws].name.trim_start_matches("special:").to_string();
                self.focused_monitor = self.workspaces[&ws].monitor;
                let fx = self.toggle_special(&WsSel::Special(Some(name)));
                self.effects.extend(fx);
            } else {
                self.focused_monitor = self.workspaces[&ws].monitor;
                let fx = self.switch_workspace(&WsSel::Id(ws));
                self.effects.extend(fx);
            }
        }
        self.emitted_focus = Some(id); // the OS already did it
        self.set_focus(Some(id));
        self.emitted_focus = Some(id);
        self.sync()
    }

    fn set_focus(&mut self, id: Option<WinId>) {
        if let Some(id) = id {
            let Some(w) = self.windows.get(&id) else { return };
            let ws = w.ws;
            self.history.retain(|&x| x != id);
            self.history.push(id);
            if let Some(wsx) = self.workspaces.get_mut(&ws) {
                wsx.last_focus = Some(id);
                self.focused_monitor = wsx.monitor;
            }
            // Focusing a window under a fullscreen one.
            if let Some(wsx) = self.workspaces.get_mut(&ws) {
                if let Some((fw, mode)) = wsx.fullscreen {
                    if fw != id && !self.windows[&id].floating {
                        match self.cfg.on_focus_under_fullscreen {
                            1 => {
                                wsx.fullscreen = Some((id, mode));
                                self.effects.push(Effect::SetFullscreen { id: fw, on: false });
                                self.effects.push(Effect::SetFullscreen { id, on: true });
                            }
                            2 => {
                                wsx.fullscreen = None;
                                self.effects.push(Effect::SetFullscreen { id: fw, on: false });
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        self.focus = id;
    }

    // -------------------------------------------------------------- geometry

    fn is_visible_ws(&self, ws: WsId) -> bool {
        self.monitors.iter().any(|m| m.active == ws || m.special == Some(ws))
    }

    fn gaps_out(&self, ws: WsId) -> Gaps {
        let mon = &self.monitors[self.workspaces[&ws].monitor];
        self.workspaces[&ws].gaps_out.unwrap_or(self.cfg.gaps_out).scaled(mon.spec.scale)
    }

    fn gaps_in(&self, ws: WsId) -> Gaps {
        let mon = &self.monitors[self.workspaces[&ws].monitor];
        self.workspaces[&ws].gaps_in.unwrap_or(self.cfg.gaps_in).scaled(mon.spec.scale)
    }

    /// Area the layout tiles into (work area minus outer gaps; scaled for special).
    pub fn layout_area(&self, ws: WsId) -> Rect {
        let Some(w) = self.workspaces.get(&ws) else { return Rect::default() };
        let mon = &self.monitors[w.monitor.min(self.monitors.len() - 1)];
        let mut area = shrink(mon.spec.work, self.gaps_out(ws));
        if ws < 0 {
            let s = self.cfg.dwindle.special_scale_factor.clamp(0.1, 1.0);
            area = area.scaled(s);
        }
        area
    }

    /// Final visible rects of tiled windows on a workspace (gaps applied).
    pub fn tiled_rects(&self, ws: WsId) -> Vec<(WinId, Rect)> {
        let Some(w) = self.workspaces.get(&ws) else { return Vec::new() };
        let area = self.layout_area(ws);
        let gin = self.gaps_in(ws);
        let b = self.cfg.border_reserve;
        // A tile must hold the window's minimum plus its inner gaps and border.
        let pad = (gin.left + gin.right + 2 * b, gin.top + gin.bottom + 2 * b);
        let min = |id: WinId| -> (i32, i32) {
            match self.windows.get(&id) {
                Some(x) if x.min_size != (0, 0) => (x.min_size.0 + pad.0, x.min_size.1 + pad.1),
                _ => (0, 0),
            }
        };
        let raw = match &w.layout {
            Layout::Dwindle(d) => d.layout_min(area, &self.cfg.dwindle, &min),
            Layout::Master(m) => m.layout(area, &self.cfg.master),
        };
        raw.into_iter()
            .map(|(id, r)| {
                let mut r = apply_inner_gaps(r, area, gin);
                r = shrink(r, Gaps::all(b));
                if let Some(win) = self.windows.get(&id) {
                    if win.pseudo {
                        let (nw, nh) = (win.natural.0.min(r.w), win.natural.1.min(r.h));
                        r = Rect::new(r.x + (r.w - nw) / 2, r.y + (r.h - nh) / 2, nw, nh);
                    }
                }
                (id, r)
            })
            .collect()
    }

    /// Where a window should be right now, if it is visible.
    pub fn desired_rect(&self, id: WinId) -> Option<Rect> {
        let w = self.windows.get(&id)?;
        if w.minimized || !self.is_visible_ws(w.ws) {
            return None;
        }
        let ws = &self.workspaces[&w.ws];
        if let Some((fw, mode)) = ws.fullscreen {
            if fw == id {
                let mon = &self.monitors[ws.monitor];
                return Some(match mode {
                    FullscreenMode::Fullscreen => mon.spec.rect,
                    FullscreenMode::Maximized => shrink(mon.spec.work, self.gaps_out(w.ws)),
                });
            }
        }
        if w.floating {
            return Some(w.float_rect);
        }
        self.tiled_rects(w.ws).into_iter().find(|(x, _)| *x == id).map(|(_, r)| r)
    }

    /// Visible windows with their rects (for directional focus), floating last.
    fn visible_rects(&self) -> Vec<(WinId, Rect)> {
        let mut v: Vec<(WinId, Rect)> = self
            .windows
            .values()
            .filter(|w| !w.minimized && self.is_visible_ws(w.ws))
            .filter_map(|w| self.desired_rect(w.id).map(|r| (w.id, r)))
            .collect();
        v.sort_by_key(|(id, _)| self.windows[id].floating);
        v
    }

    // -------------------------------------------------------------- sync

    /// Diffs desired state against what was last emitted.
    pub fn sync(&mut self) -> Vec<Effect> {
        let ids: Vec<WinId> = self.windows.keys().copied().collect();
        let mut shows = Vec::new();
        let mut hides = Vec::new();
        for id in ids {
            let w = &self.windows[&id];
            let visible = !w.minimized && self.is_visible_ws(w.ws);
            if w.minimized {
                continue;
            }
            if visible {
                if let Some(r) = self.desired_rect(id) {
                    if self.windows[&id].placed != Some(r) {
                        self.effects.push(Effect::Place { id, rect: r, anim: Anim::Move });
                        self.windows.get_mut(&id).unwrap().placed = Some(r);
                    }
                }
                if !self.windows[&id].shown {
                    shows.push(id);
                    self.windows.get_mut(&id).unwrap().shown = true;
                }
            } else if self.windows[&id].shown {
                hides.push(id);
                self.windows.get_mut(&id).unwrap().shown = false;
            }
        }
        self.effects.extend(hides.into_iter().map(Effect::Hide));
        self.effects.extend(shows.into_iter().map(Effect::Show));
        // Special workspaces and fullscreen/floating windows stay on top.
        for m in &self.monitors {
            if let Some(s) = m.special {
                for w in self.windows.values().filter(|w| w.ws == s && !w.minimized) {
                    self.effects.push(Effect::Raise(w.id));
                }
            }
        }
        if self.focus != self.emitted_focus {
            if let Some(old) = self.emitted_focus {
                if self.windows.contains_key(&old) {
                    self.effects.push(Effect::Border { id: old, active: false });
                }
            }
            if let Some(f) = self.focus {
                self.effects.push(Effect::Focus(f));
                self.effects.push(Effect::Border { id: f, active: true });
                self.effects.push(Effect::Event { name: "activewindowv2", data: format!("{f:x}") });
            }
            self.emitted_focus = self.focus;
        }
        std::mem::take(&mut self.effects)
    }

    /// Records the smallest size an app accepts and re-tiles around it.
    pub fn set_min_size(&mut self, id: WinId, w: i32, h: i32) -> Vec<Effect> {
        if let Some(win) = self.windows.get_mut(&id) {
            win.min_size = (win.min_size.0.max(w), win.min_size.1.max(h));
        }
        self.invalidate_all();
        self.sync()
    }

    /// Forget what was placed so the next sync re-applies it (app moved itself).
    pub fn invalidate(&mut self, id: WinId) {
        if let Some(w) = self.windows.get_mut(&id) {
            w.placed = None;
        }
    }

    pub fn invalidate_all(&mut self) {
        for w in self.windows.values_mut() {
            w.placed = None;
        }
    }

    // -------------------------------------------------------------- dispatchers

    pub fn focus_dir(&mut self, dir: Dir) -> Vec<Effect> {
        let rects = self.visible_rects();
        let from = self.focus.and_then(|f| rects.iter().find(|(w, _)| *w == f).map(|x| x.1));
        let from = from.unwrap_or_else(|| self.monitors[self.focused_monitor].spec.rect.scaled(0.01));
        // Prefer windows on the current monitor, then neighbours across monitors.
        if let Some(t) = neighbor(from, dir, &rects) {
            self.set_focus(Some(t));
            return self.sync();
        }
        let mons: Vec<(MonId, Rect)> = self.monitors.iter().enumerate().map(|(i, m)| (i, m.spec.rect)).collect();
        if let Some(m) = neighbor(self.monitors[self.focused_monitor].spec.rect, dir, &mons) {
            self.focus_monitor_idx(m);
        }
        self.sync()
    }

    fn focus_monitor_idx(&mut self, m: MonId) {
        self.focused_monitor = m;
        let ws = self.monitors[m].special.unwrap_or(self.monitors[m].active);
        let f = self.workspaces.get(&ws).and_then(|w| w.last_focus).or_else(|| self.first_window_of(ws));
        self.focus = f;
        let r = self.monitors[m].spec.rect;
        self.effects.push(Effect::WarpCursor { x: r.x + r.w / 2, y: r.y + r.h / 2 });
        self.effects.push(Effect::Event { name: "focusedmon", data: format!("{},{}", self.monitors[m].spec.name, self.workspaces.get(&ws).map(|w| w.name.clone()).unwrap_or_default()) });
    }

    pub fn focus_monitor(&mut self, sel: &str) -> Vec<Effect> {
        let m = match Dir::parse(sel) {
            Some(d) => {
                let mons: Vec<(MonId, Rect)> = self.monitors.iter().enumerate().map(|(i, m)| (i, m.spec.rect)).collect();
                neighbor(self.monitors[self.focused_monitor].spec.rect, d, &mons)
            }
            None => match sel {
                "+1" => Some((self.focused_monitor + 1) % self.monitors.len()),
                "-1" => Some((self.focused_monitor + self.monitors.len() - 1) % self.monitors.len()),
                _ => self.monitors.iter().position(|m| m.spec.name == sel).or_else(|| sel.parse::<usize>().ok().filter(|i| *i < self.monitors.len())),
            },
        };
        if let Some(m) = m {
            self.focus_monitor_idx(m);
        }
        self.sync()
    }

    pub fn focus_window(&mut self, id: WinId) -> Vec<Effect> {
        if self.windows.contains_key(&id) {
            return self.on_os_focus_internal(id);
        }
        self.sync()
    }

    fn on_os_focus_internal(&mut self, id: WinId) -> Vec<Effect> {
        let ws = self.windows[&id].ws;
        if !self.is_visible_ws(ws) && ws > 0 {
            self.focused_monitor = self.workspaces[&ws].monitor;
            let fx = self.switch_workspace(&WsSel::Id(ws));
            self.effects.extend(fx);
        }
        self.set_focus(Some(id));
        self.sync()
    }

    /// `movewindow <dir>`: tiled windows move within the layout, floating
    /// windows jump to the monitor edge; at the edge they cross monitors.
    pub fn move_dir(&mut self, dir: Dir) -> Vec<Effect> {
        let Some(f) = self.focus else { return self.sync() };
        let Some(w) = self.windows.get(&f).cloned() else { return self.sync() };
        if w.floating {
            let mon = &self.monitors[self.workspaces[&w.ws].monitor].spec;
            let area = shrink(mon.work, self.gaps_out(w.ws));
            let mut r = w.float_rect;
            match dir {
                Dir::Left => r.x = area.x,
                Dir::Right => r.x = area.right() - r.w,
                Dir::Up => r.y = area.y,
                Dir::Down => r.y = area.bottom() - r.h,
            }
            self.windows.get_mut(&f).unwrap().float_rect = r;
            return self.sync();
        }
        let rects = self.tiled_rects(w.ws);
        let from = rects.iter().find(|x| x.0 == f).map(|x| x.1);
        let target = from.and_then(|fr| neighbor(fr, dir, &rects));
        match target {
            Some(t) => {
                let dcfg = self.cfg.dwindle.clone();
                let ws = self.workspaces.get_mut(&w.ws).unwrap();
                match &mut ws.layout {
                    Layout::Dwindle(d) => d.move_next_to(f, t, dir, &dcfg),
                    Layout::Master(m) => m.swap(f, t),
                }
            }
            None => {
                // Cross to the neighbouring monitor's active workspace.
                let mons: Vec<(MonId, Rect)> = self.monitors.iter().enumerate().map(|(i, m)| (i, m.spec.rect)).collect();
                let cur = self.workspaces[&w.ws].monitor;
                if let Some(m) = neighbor(self.monitors[cur].spec.rect, dir, &mons) {
                    let target_ws = self.monitors[m].active;
                    self.detach(f);
                    self.windows.get_mut(&f).unwrap().ws = target_ws;
                    self.focused_monitor = m;
                    self.attach(f, None);
                    self.set_focus(Some(f));
                    self.gc_workspaces();
                }
            }
        }
        self.sync()
    }

    pub fn swap_dir(&mut self, dir: Dir) -> Vec<Effect> {
        let Some(f) = self.focus else { return self.sync() };
        let Some(ws) = self.windows.get(&f).map(|w| w.ws) else { return self.sync() };
        let rects = self.tiled_rects(ws);
        let Some(from) = rects.iter().find(|x| x.0 == f).map(|x| x.1) else { return self.sync() };
        if let Some(t) = neighbor(from, dir, &rects) {
            match &mut self.workspaces.get_mut(&ws).unwrap().layout {
                Layout::Dwindle(d) => d.swap(f, t),
                Layout::Master(m) => m.swap(f, t),
            }
        }
        self.sync()
    }

    pub fn set_floating(&mut self, win: Option<WinId>, floating: Option<bool>) -> Vec<Effect> {
        let Some(id) = win.or(self.focus) else { return self.sync() };
        let Some(w) = self.windows.get(&id).cloned() else { return self.sync() };
        let new = floating.unwrap_or(!w.floating);
        if new == w.floating {
            return self.sync();
        }
        // An explicit choice by the user overrides automatic floating.
        self.windows.get_mut(&id).unwrap().auto_float = false;
        if new {
            let current = self.desired_rect(id).unwrap_or(w.float_rect);
            self.detach(id);
            let win = self.windows.get_mut(&id).unwrap();
            win.floating = true;
            // Hyprland keeps the tiled geometry when floating a window; use a
            // slightly smaller copy so the change is visible.
            if win.float_rect == Rect::default() || win.placed.is_some() {
                win.float_rect = current.scaled(0.9);
            }
        } else {
            let win = self.windows.get_mut(&id).unwrap();
            win.floating = false;
            win.pinned = false;
            self.attach(id, None);
        }
        if new {
            // Floating this window freed space for others.
            self.retry_auto_floats(w.ws);
        }
        self.effects.push(Effect::Event { name: "changefloatingmode", data: format!("{id:x},{}", new as u8) });
        self.sync()
    }

    /// Floats a window that cannot fit its minimum size on its workspace. It
    /// returns to tiling automatically when space frees up or when it is moved
    /// to another workspace.
    pub fn auto_float(&mut self, id: WinId) -> Vec<Effect> {
        let Some(w) = self.windows.get(&id).cloned() else { return self.sync() };
        if w.floating {
            return self.sync();
        }
        self.detach(id);
        let mon = self.workspaces.get(&w.ws).map(|x| x.monitor).unwrap_or(0).min(self.monitors.len() - 1);
        let work = self.monitors[mon].spec.work;
        let size = Rect::new(0, 0, w.min_size.0.max(w.natural.0).min(work.w), w.min_size.1.max(w.natural.1).min(work.h));
        let win = self.windows.get_mut(&id).unwrap();
        win.floating = true;
        win.auto_float = true;
        win.float_rect = center_in(size, work);
        self.effects.push(Effect::Event { name: "changefloatingmode", data: format!("{id:x},1") });
        self.sync()
    }

    /// A window that cannot fit its minimum size where it is: move it (silently)
    /// to the next workspace on the same monitor where everything still fits,
    /// creating one if needed. Falls back to floating when it fits nowhere,
    /// not even alone. Returns the workspace it went to, if it moved.
    pub fn overflow(&mut self, id: WinId) -> (Vec<Effect>, Option<WsId>) {
        let Some(w) = self.windows.get(&id).cloned() else { return (self.sync(), None) };
        if w.floating || w.ws < 0 {
            return (self.sync(), None);
        }
        let src = w.ws;
        let mon = self.workspaces.get(&src).map(|x| x.monitor).unwrap_or(self.focused_monitor);
        let was_focused = self.focus == Some(id);
        for n in (src + 1)..(src + 100) {
            // Skip workspaces that live on (or are shown on) another monitor.
            if let Some(ws) = self.workspaces.get(&n) {
                if ws.monitor != mon {
                    continue;
                }
            }
            let existed = self.workspaces.contains_key(&n);
            let fx_mark = self.effects.len();
            self.ensure_ws(n, None, mon);
            self.detach(id);
            self.windows.get_mut(&id).unwrap().ws = n;
            self.attach(id, None);
            if self.ws_fits(n) {
                let name = self.workspaces[&n].name.clone();
                self.effects.push(Effect::Event { name: "movewindow", data: format!("{id:x},{name}") });
                self.effects.push(Effect::Event { name: "movewindowv2", data: format!("{id:x},{n},{name}") });
                if was_focused {
                    self.focus = self.first_window_of(src);
                }
                self.gc_workspaces();
                return (self.sync(), Some(n));
            }
            // Undo the trial.
            self.detach(id);
            self.windows.get_mut(&id).unwrap().ws = src;
            if !existed {
                // Drop the trial workspace and its creation event. An empty
                // workspace was not enough, so it fits nowhere: stop looking.
                self.workspaces.remove(&n);
                self.effects.truncate(fx_mark);
                break;
            }
        }
        self.attach(id, None);
        (self.auto_float(id), None)
    }

    /// Makes workspace `ws` fit again by moving its most recently opened tiled
    /// windows, newest first, to the next workspace with room. The windows that
    /// were there first stay put. Returns each move as (window, workspace).
    pub fn overflow_ws(&mut self, ws: WsId) -> (Vec<Effect>, Vec<(WinId, Option<WsId>)>) {
        let mut moved = Vec::new();
        let mut fx = Vec::new();
        for _ in 0..16 {
            if self.ws_fits(ws) {
                break;
            }
            let tiled = self.workspaces.get(&ws).map(|w| w.layout.windows()).unwrap_or_default();
            if tiled.len() <= 1 {
                break;
            }
            let Some(victim) = tiled.into_iter().max_by_key(|id| self.windows[id].seq) else { break };
            let (f, to) = self.overflow(victim);
            fx.extend(f);
            moved.push((victim, to));
        }
        fx.extend(self.sync());
        (fx, moved)
    }

    /// Sets the "age" order of windows, oldest first. Used at startup so the
    /// windows the user touched most recently count as the oldest and are
    /// the last to be moved away by [`overflow_ws`](Self::overflow_ws).
    pub fn set_age_order(&mut self, oldest_first: &[WinId]) {
        for (i, id) in oldest_first.iter().enumerate() {
            if let Some(w) = self.windows.get_mut(id) {
                w.seq = i as u64 + 1;
            }
        }
    }

    /// True when every tiled window on `ws` gets at least its minimum size.
    fn ws_fits(&self, ws: WsId) -> bool {
        self.tiled_rects(ws).iter().all(|(id, r)| {
            let m = self.windows.get(id).map(|w| w.min_size).unwrap_or((0, 0));
            r.w + 2 >= m.0 && r.h + 2 >= m.1
        })
    }

    /// Tries to tile auto-floated windows on `ws` again, one at a time.
    fn retry_auto_floats(&mut self, ws: WsId) {
        let candidates: Vec<WinId> = self.windows.values().filter(|w| w.ws == ws && w.auto_float && w.floating && !w.minimized).map(|w| w.id).collect();
        for id in candidates {
            self.windows.get_mut(&id).unwrap().floating = false;
            self.attach(id, None);
            if self.ws_fits(ws) {
                self.windows.get_mut(&id).unwrap().auto_float = false;
                self.effects.push(Effect::Event { name: "changefloatingmode", data: format!("{id:x},0") });
            } else {
                self.detach(id);
                self.windows.get_mut(&id).unwrap().floating = true;
            }
        }
    }

    pub fn fullscreen(&mut self, mode: FullscreenMode, action: Option<bool>) -> Vec<Effect> {
        let Some(id) = self.focus else { return self.sync() };
        let Some(ws) = self.windows.get(&id).map(|w| w.ws) else { return self.sync() };
        let wsx = self.workspaces.get_mut(&ws).unwrap();
        let currently = wsx.fullscreen.filter(|f| f.0 == id);
        let turn_on = action.unwrap_or(currently.map(|f| f.1 != mode).unwrap_or(true));
        if let Some((old, _)) = wsx.fullscreen.take() {
            self.effects.push(Effect::SetFullscreen { id: old, on: false });
        }
        if turn_on {
            self.workspaces.get_mut(&ws).unwrap().fullscreen = Some((id, mode));
            self.effects.push(Effect::SetFullscreen { id, on: mode == FullscreenMode::Fullscreen });
            self.effects.push(Effect::Raise(id));
        }
        self.effects.push(Effect::Event { name: "fullscreen", data: (turn_on as u8).to_string() });
        self.sync()
    }

    pub fn pin(&mut self, win: Option<WinId>) -> Vec<Effect> {
        let Some(id) = win.or(self.focus) else { return self.sync() };
        let Some(w) = self.windows.get_mut(&id) else { return self.sync() };
        if !w.floating {
            return self.sync(); // Hyprland: only floating windows can be pinned
        }
        w.pinned = !w.pinned;
        let p = w.pinned;
        self.effects.push(Effect::Event { name: "pin", data: format!("{id:x},{}", p as u8) });
        self.sync()
    }

    pub fn pseudo(&mut self, win: Option<WinId>) -> Vec<Effect> {
        let Some(id) = win.or(self.focus) else { return self.sync() };
        if let Some(w) = self.windows.get_mut(&id) {
            w.pseudo = !w.pseudo;
        }
        self.sync()
    }

    /// `resizeactive dx dy` (pixels; `exact` sets the size for floating windows).
    pub fn resize_active(&mut self, dx: i32, dy: i32, exact: bool) -> Vec<Effect> {
        let Some(id) = self.focus else { return self.sync() };
        let Some(w) = self.windows.get(&id).cloned() else { return self.sync() };
        if w.floating {
            let r = &mut self.windows.get_mut(&id).unwrap().float_rect;
            if exact {
                r.w = dx.max(50);
                r.h = dy.max(50);
            } else {
                r.w = (r.w + dx).max(50);
                r.h = (r.h + dy).max(50);
            }
            return self.sync();
        }
        let area = self.layout_area(w.ws);
        let (dcfg, mcfg) = (self.cfg.dwindle.clone(), self.cfg.master.clone());
        match &mut self.workspaces.get_mut(&w.ws).unwrap().layout {
            Layout::Dwindle(d) => d.resize(id, dx, dy, area, &dcfg),
            Layout::Master(m) => m.resize(id, dx, dy, area, &mcfg),
        }
        self.sync()
    }

    /// `moveactive dx dy` (floating only, like Hyprland).
    pub fn move_active(&mut self, dx: i32, dy: i32, exact: bool) -> Vec<Effect> {
        let Some(id) = self.focus else { return self.sync() };
        if let Some(w) = self.windows.get_mut(&id) {
            if w.floating {
                if exact {
                    w.float_rect.x = dx;
                    w.float_rect.y = dy;
                } else {
                    w.float_rect = w.float_rect.offset(dx, dy);
                }
            }
        }
        self.sync()
    }

    pub fn center_window(&mut self) -> Vec<Effect> {
        let Some(id) = self.focus else { return self.sync() };
        let Some(w) = self.windows.get(&id).cloned() else { return self.sync() };
        if w.floating {
            let mon = &self.monitors[self.workspaces[&w.ws].monitor].spec;
            self.windows.get_mut(&id).unwrap().float_rect = center_in(w.float_rect, mon.work);
        }
        self.sync()
    }

    pub fn cycle(&mut self, forward: bool, tiled_only: bool, floating_only: bool) -> Vec<Effect> {
        let mon = self.focused_monitor;
        let ws = self.monitors[mon].special.unwrap_or(self.monitors[mon].active);
        let mut list: Vec<WinId> = match self.workspaces.get(&ws) {
            Some(w) => w.layout.windows(),
            None => Vec::new(),
        };
        list.extend(self.windows.values().filter(|w| w.ws == ws && w.floating && !w.minimized).map(|w| w.id));
        list.retain(|id| {
            let fl = self.windows[id].floating;
            !(tiled_only && fl) && !(floating_only && !fl)
        });
        if list.is_empty() {
            return self.sync();
        }
        let i = self.focus.and_then(|f| list.iter().position(|&x| x == f));
        let next = match i {
            Some(i) => list[(i as i64 + if forward { 1 } else { -1 }).rem_euclid(list.len() as i64) as usize],
            None => list[0],
        };
        self.set_focus(Some(next));
        self.sync()
    }

    pub fn swap_next(&mut self, forward: bool) -> Vec<Effect> {
        let Some(f) = self.focus else { return self.sync() };
        let Some(ws) = self.windows.get(&f).map(|w| w.ws) else { return self.sync() };
        let list = self.workspaces[&ws].layout.windows();
        let Some(i) = list.iter().position(|&x| x == f) else { return self.sync() };
        if list.len() < 2 {
            return self.sync();
        }
        let t = list[(i as i64 + if forward { 1 } else { -1 }).rem_euclid(list.len() as i64) as usize];
        match &mut self.workspaces.get_mut(&ws).unwrap().layout {
            Layout::Dwindle(d) => d.swap(f, t),
            Layout::Master(m) => m.swap(f, t),
        }
        self.sync()
    }

    pub fn focus_current_or_last(&mut self) -> Vec<Effect> {
        let prev = self.history.iter().rev().copied().find(|&w| Some(w) != self.focus && self.windows.get(&w).is_some_and(|x| !x.minimized));
        if let Some(p) = prev {
            return self.on_os_focus_internal(p);
        }
        self.sync()
    }

    /// `layoutmsg …`
    pub fn layout_msg(&mut self, msg: &str) -> Vec<Effect> {
        let Some(f) = self.focus else { return self.sync() };
        let Some(ws) = self.windows.get(&f).map(|w| w.ws) else { return self.sync() };
        let area = self.layout_area(ws);
        let (dcfg, mcfg) = (self.cfg.dwindle.clone(), self.cfg.master.clone());
        let mut parts = msg.split_whitespace();
        let cmd = parts.next().unwrap_or("");
        match &mut self.workspaces.get_mut(&ws).unwrap().layout {
            Layout::Dwindle(d) => match cmd {
                "togglesplit" => d.toggle_split(f, area, &dcfg),
                "swapsplit" => d.swap_split(f),
                "preselect" => d.preselect(parts.next().and_then(Dir::parse)),
                "splitratio" => {
                    if let Some(v) = parts.next().and_then(|v| v.parse::<f64>().ok()) {
                        let exact = parts.next() == Some("exact");
                        d.split_ratio(f, v, exact);
                    }
                }
                _ => {}
            },
            Layout::Master(m) => {
                if cmd == "focusmaster" {
                    let mw = m.master_window();
                    if mw.is_some() {
                        self.set_focus(mw);
                    }
                } else if cmd == "cyclenext" || cmd == "cycleprev" {
                    let n = m.neighbour_in_order(f, cmd == "cyclenext");
                    self.set_focus(n);
                } else if cmd == "swapnext" || cmd == "swapprev" {
                    if let Some(n) = m.neighbour_in_order(f, cmd == "swapnext") {
                        m.swap(f, n);
                    }
                } else {
                    m.message(msg, Some(f), &mcfg);
                }
            }
        }
        self.sync()
    }

    /// Changes a workspace's layout (`workspace` rule or keyword at runtime).
    pub fn set_layout(&mut self, ws: WsId, kind: LayoutKind) -> Vec<Effect> {
        let Some(w) = self.workspaces.get(&ws) else { return self.sync() };
        if w.layout.kind() == kind {
            return self.sync();
        }
        let wins = w.layout.windows();
        self.workspaces.get_mut(&ws).unwrap().layout = Layout::new(kind);
        for id in wins {
            self.attach(id, None);
        }
        self.sync()
    }

    pub fn close_active(&mut self) -> Vec<Effect> {
        let mut fx = self.sync();
        if let Some(f) = self.focus {
            fx.push(Effect::Close(f));
        }
        fx
    }

    pub fn kill_active(&mut self) -> Vec<Effect> {
        let mut fx = self.sync();
        if let Some(f) = self.focus {
            fx.push(Effect::Kill(f));
        }
        fx
    }

    // -------------------------------------------------------------- mouse

    /// The user dragged a window (Super+LMB or title bar) and released at `cursor`.
    /// Floating windows keep the new rect; tiled windows are dropped next to the
    /// tiled window under the cursor (or snap back).
    pub fn on_user_move(&mut self, id: WinId, rect: Rect, cursor: (f64, f64)) -> Vec<Effect> {
        let Some(w) = self.windows.get(&id).cloned() else { return self.sync() };
        let target_mon = self.monitor_at(cursor.0, cursor.1).unwrap_or(self.focused_monitor);
        if w.floating {
            self.windows.get_mut(&id).unwrap().float_rect = rect;
            let ws_mon = self.workspaces[&w.ws].monitor;
            if target_mon != ws_mon && w.ws > 0 {
                let target_ws = self.monitors[target_mon].active;
                self.windows.get_mut(&id).unwrap().ws = target_ws;
            }
            self.windows.get_mut(&id).unwrap().placed = Some(rect);
            return self.sync();
        }
        let target_ws = self.monitors[target_mon].special.unwrap_or(self.monitors[target_mon].active);
        let rects = self.tiled_rects(target_ws);
        let under = rects.iter().find(|(t, r)| *t != id && contains_point(*r, cursor.0, cursor.1)).copied();
        self.invalidate(id);
        match under {
            Some((t, r)) => {
                // Side = the edge of the target nearest to the cursor.
                let (cx, cy) = r.center();
                let (dx, dy) = ((cursor.0 - cx) / r.w as f64, (cursor.1 - cy) / r.h as f64);
                let dir = if dx.abs() > dy.abs() {
                    if dx < 0.0 { Dir::Left } else { Dir::Right }
                } else if dy < 0.0 {
                    Dir::Up
                } else {
                    Dir::Down
                };
                self.detach(id);
                self.windows.get_mut(&id).unwrap().ws = target_ws;
                let (dcfg, mcfg) = (self.cfg.dwindle.clone(), self.cfg.master.clone());
                let area = self.layout_area(target_ws);
                match &mut self.workspaces.get_mut(&target_ws).unwrap().layout {
                    Layout::Dwindle(d) => {
                        d.insert(id, Some(t), None, area, &dcfg);
                        d.move_next_to(id, t, dir, &dcfg);
                    }
                    Layout::Master(m) => {
                        m.insert(id, Some(t), &mcfg);
                        m.swap(id, t);
                    }
                }
            }
            None if target_ws != w.ws => {
                self.detach(id);
                self.windows.get_mut(&id).unwrap().ws = target_ws;
                self.attach(id, Some(cursor));
            }
            None => {}
        }
        self.set_focus(Some(id));
        self.gc_workspaces();
        self.sync()
    }

    /// The user resized a window with the mouse to `rect`.
    pub fn on_user_resize(&mut self, id: WinId, rect: Rect) -> Vec<Effect> {
        let Some(w) = self.windows.get(&id).cloned() else { return self.sync() };
        if w.floating {
            let win = self.windows.get_mut(&id).unwrap();
            win.float_rect = rect;
            win.placed = Some(rect);
            return self.sync();
        }
        let Some(cur) = self.desired_rect(id) else { return self.sync() };
        // Translate edge movement into layout deltas.
        let dx = if rect.x != cur.x { cur.x - rect.x } else { rect.right() - cur.right() };
        let dy = if rect.y != cur.y { cur.y - rect.y } else { rect.bottom() - cur.bottom() };
        self.invalidate(id);
        let area = self.layout_area(w.ws);
        let (dcfg, mcfg) = (self.cfg.dwindle.clone(), self.cfg.master.clone());
        match &mut self.workspaces.get_mut(&w.ws).unwrap().layout {
            Layout::Dwindle(d) => d.resize(id, dx, dy, area, &dcfg),
            Layout::Master(m) => m.resize(id, dx, dy, area, &mcfg),
        }
        self.invalidate_all();
        self.sync()
    }

    // -------------------------------------------------------------- queries

    pub fn active_workspace(&self) -> WsId {
        let m = &self.monitors[self.focused_monitor];
        m.special.unwrap_or(m.active)
    }

    pub fn is_tiled(&self, id: WinId) -> bool {
        self.windows.get(&id).is_some_and(|w| !w.floating && self.workspaces.get(&w.ws).is_some_and(|ws| ws.layout.contains(id)))
    }

    pub fn fullscreen_of(&self, id: WinId) -> Option<FullscreenMode> {
        let w = self.windows.get(&id)?;
        self.workspaces.get(&w.ws)?.fullscreen.filter(|f| f.0 == id).map(|f| f.1)
    }

    /// Windows the WM has hidden (for the crash ledger).
    pub fn hidden_windows(&self) -> Vec<WinId> {
        self.windows.values().filter(|w| !w.shown).map(|w| w.id).collect()
    }
}

fn rect_inside(r: Rect, outer: Rect) -> bool {
    let (cx, cy) = r.center();
    contains_point(outer, cx, cy)
}

fn center_in(r: Rect, area: Rect) -> Rect {
    let w = r.w.min(area.w);
    let h = r.h.min(area.h);
    Rect::new(area.x + (area.w - w) / 2, area.y + (area.h - h) / 2, w, h)
}

// ------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;

    fn mon(name: &str, x: i32) -> MonitorSpec {
        MonitorSpec { name: name.into(), rect: Rect::new(x, 0, 1920, 1080), work: Rect::new(x, 0, 1920, 1080), scale: 1.0 }
    }

    fn wm1() -> Wm {
        Wm::new(WmConfig { gaps_in: Gaps::all(5), gaps_out: Gaps::all(20), ..Default::default() }, vec![mon("A", 0)])
    }

    fn tiled(rect: Rect) -> NewWindow {
        NewWindow { rect, ..Default::default() }
    }

    fn places(fx: &[Effect]) -> Vec<(WinId, Rect)> {
        fx.iter().filter_map(|e| if let Effect::Place { id, rect, .. } = e { Some((*id, *rect)) } else { None }).collect()
    }

    #[test]
    fn hyprland_default_gaps_are_pixel_exact() {
        let mut wm = wm1();
        let fx = wm.add_window(1, tiled(Rect::new(100, 100, 400, 300)));
        assert_eq!(places(&fx), vec![(1, Rect::new(20, 20, 1880, 1040))]);
        let fx = wm.add_window(2, tiled(Rect::new(100, 100, 400, 300)));
        let p = places(&fx);
        let r1 = p.iter().find(|x| x.0 == 1).unwrap().1;
        let r2 = p.iter().find(|x| x.0 == 2).unwrap().1;
        assert_eq!(r1, Rect::new(20, 20, 935, 1040));
        assert_eq!(r2.x - r1.right(), 10, "gaps_in on both sides");
        assert_eq!(r2.right(), 1900, "gaps_out");
    }

    #[test]
    fn new_window_takes_focus_and_borders_follow() {
        let mut wm = wm1();
        wm.add_window(1, tiled(Rect::default()));
        let fx = wm.add_window(2, tiled(Rect::default()));
        assert!(fx.contains(&Effect::Focus(2)));
        assert!(fx.contains(&Effect::Border { id: 1, active: false }));
        assert!(fx.contains(&Effect::Border { id: 2, active: true }));
    }

    #[test]
    fn sync_is_idempotent() {
        let mut wm = wm1();
        wm.add_window(1, tiled(Rect::default()));
        assert!(wm.sync().is_empty());
    }

    #[test]
    fn workspace_switch_hides_and_shows() {
        let mut wm = wm1();
        wm.add_window(1, tiled(Rect::default()));
        let fx = wm.switch_workspace(&WsSel::Id(2));
        assert!(fx.contains(&Effect::Hide(1)));
        wm.add_window(2, tiled(Rect::default()));
        let fx = wm.switch_workspace(&WsSel::Id(1));
        assert!(fx.contains(&Effect::Hide(2)));
        assert!(fx.contains(&Effect::Show(1)));
        assert!(fx.contains(&Effect::Focus(1)));
    }

    #[test]
    fn empty_workspaces_are_destroyed() {
        let mut wm = wm1();
        wm.switch_workspace(&WsSel::Id(3));
        wm.switch_workspace(&WsSel::Id(1));
        assert!(!wm.workspaces.contains_key(&3));
    }

    #[test]
    fn move_to_workspace_silent_and_follow() {
        let mut wm = wm1();
        wm.add_window(1, tiled(Rect::default()));
        wm.add_window(2, tiled(Rect::default()));
        let fx = wm.move_to_workspace(None, &WsSel::Id(4), false);
        assert!(fx.contains(&Effect::Hide(2)));
        assert_eq!(wm.active_workspace(), 1);
        assert_eq!(wm.focus, Some(1));
        let fx = wm.move_to_workspace(Some(1), &WsSel::Id(4), true);
        assert_eq!(wm.active_workspace(), 4);
        assert!(fx.contains(&Effect::Focus(1)) || wm.focus == Some(1));
        assert_eq!(wm.workspaces[&4].layout.windows().len(), 2);
    }

    #[test]
    fn special_workspace_toggles_as_overlay() {
        let mut wm = wm1();
        wm.add_window(1, tiled(Rect::default()));
        wm.move_to_workspace(None, &WsSel::Special(Some("magic".into())), false);
        assert!(!wm.windows[&1].shown);
        let fx = wm.toggle_special(&WsSel::Special(Some("magic".into())));
        assert!(fx.contains(&Effect::Show(1)));
        assert!(fx.contains(&Effect::Raise(1)));
        let fx = wm.toggle_special(&WsSel::Special(Some("magic".into())));
        assert!(fx.contains(&Effect::Hide(1)));
    }

    #[test]
    fn workspace_selectors() {
        assert_eq!(WsSel::parse("3"), Some(WsSel::Id(3)));
        assert_eq!(WsSel::parse("+1"), Some(WsSel::Relative(1)));
        assert_eq!(WsSel::parse("e-1"), Some(WsSel::OpenRelative(-1)));
        assert_eq!(WsSel::parse("m+1"), Some(WsSel::MonitorRelative(1)));
        assert_eq!(WsSel::parse("r+2"), Some(WsSel::Relative(2)));
        assert_eq!(WsSel::parse("special:magic"), Some(WsSel::Special(Some("magic".into()))));
        assert_eq!(WsSel::parse("name:web"), Some(WsSel::Name("web".into())));
        assert_eq!(WsSel::parse("previous"), Some(WsSel::Previous));
    }

    #[test]
    fn relative_and_previous() {
        let mut wm = wm1();
        wm.switch_workspace(&WsSel::Relative(1));
        assert_eq!(wm.active_workspace(), 2);
        wm.switch_workspace(&WsSel::Id(5));
        wm.switch_workspace(&WsSel::Previous);
        assert_eq!(wm.active_workspace(), 2);
    }

    #[test]
    fn back_and_forth() {
        let mut wm = wm1();
        wm.cfg.workspace_back_and_forth = true;
        wm.switch_workspace(&WsSel::Id(2));
        wm.switch_workspace(&WsSel::Id(2));
        assert_eq!(wm.active_workspace(), 1);
    }

    #[test]
    fn two_monitors_get_own_workspaces_and_focus_crosses() {
        let mut wm = Wm::new(WmConfig::default(), vec![mon("A", 0), mon("B", 1920)]);
        assert_eq!(wm.monitors[0].active, 1);
        assert_eq!(wm.monitors[1].active, 2);
        wm.add_window(1, tiled(Rect::new(100, 100, 300, 300)));
        wm.add_window(2, tiled(Rect::new(2100, 100, 300, 300)));
        wm.set_focus(Some(1));
        wm.sync();
        wm.focus_dir(Dir::Right);
        assert_eq!(wm.focus, Some(2));
        // workspace 1 is visible on monitor A: switching to it from B focuses A.
        wm.switch_workspace(&WsSel::Id(1));
        assert_eq!(wm.focused_monitor, 0);
    }

    #[test]
    fn monitor_unplug_keeps_windows() {
        let mut wm = Wm::new(WmConfig::default(), vec![mon("A", 0), mon("B", 1920)]);
        wm.add_window(1, tiled(Rect::new(2100, 100, 300, 300)));
        assert_eq!(wm.windows[&1].ws, 2);
        wm.set_monitors(vec![mon("A", 0)]);
        assert!(wm.windows.contains_key(&1));
        assert_eq!(wm.workspaces[&2].monitor, 0);
        wm.switch_workspace(&WsSel::Id(2));
        assert!(wm.desired_rect(1).is_some());
    }

    #[test]
    fn floating_toggle_and_resize() {
        let mut wm = wm1();
        wm.add_window(1, tiled(Rect::default()));
        wm.add_window(2, tiled(Rect::default()));
        wm.set_floating(None, None);
        assert!(wm.windows[&2].floating);
        assert_eq!(wm.tiled_rects(1).len(), 1);
        let before = wm.windows[&2].float_rect;
        wm.resize_active(10, 20, false);
        assert_eq!(wm.windows[&2].float_rect.w, before.w + 10);
        wm.set_floating(None, Some(false));
        assert_eq!(wm.tiled_rects(1).len(), 2);
    }

    #[test]
    fn fullscreen_modes_and_unfullscreen_on_new_window() {
        let mut wm = wm1();
        wm.add_window(1, tiled(Rect::default()));
        wm.fullscreen(FullscreenMode::Fullscreen, None);
        assert_eq!(wm.desired_rect(1), Some(Rect::new(0, 0, 1920, 1080)));
        wm.fullscreen(FullscreenMode::Maximized, None);
        assert_eq!(wm.desired_rect(1), Some(Rect::new(20, 20, 1880, 1040)));
        wm.add_window(2, tiled(Rect::default()));
        assert_eq!(wm.fullscreen_of(1), None);
    }

    #[test]
    fn pin_follows_workspace_switch() {
        let mut wm = wm1();
        wm.add_window(1, NewWindow { floating: true, rect: Rect::new(100, 100, 300, 300), ..Default::default() });
        wm.pin(None);
        let fx = wm.switch_workspace(&WsSel::Id(2));
        assert!(!fx.contains(&Effect::Hide(1)));
        assert_eq!(wm.windows[&1].ws, 2);
    }

    #[test]
    fn minimize_removes_from_layout_and_restore_returns() {
        let mut wm = wm1();
        wm.add_window(1, tiled(Rect::default()));
        wm.add_window(2, tiled(Rect::default()));
        wm.set_minimized(2, true);
        assert_eq!(wm.tiled_rects(1), vec![(1, Rect::new(20, 20, 1880, 1040))]);
        assert_eq!(wm.focus, Some(1));
        wm.set_minimized(2, false);
        assert_eq!(wm.tiled_rects(1).len(), 2);
        assert_eq!(wm.focus, Some(2));
    }

    #[test]
    fn closing_focused_window_focuses_previous() {
        let mut wm = wm1();
        for i in 1..=3 {
            wm.add_window(i, tiled(Rect::default()));
        }
        wm.focus_window(1);
        wm.focus_window(3);
        wm.remove_window(3);
        assert_eq!(wm.focus, Some(1));
    }

    #[test]
    fn os_focus_on_hidden_workspace_switches_to_it() {
        let mut wm = wm1();
        wm.add_window(1, tiled(Rect::default()));
        wm.switch_workspace(&WsSel::Id(2));
        let fx = wm.on_os_focus(1);
        assert_eq!(wm.active_workspace(), 1);
        assert!(fx.contains(&Effect::Show(1)));
    }

    #[test]
    fn drag_drop_reorders_tiles() {
        let mut wm = wm1();
        wm.add_window(1, tiled(Rect::default()));
        wm.add_window(2, tiled(Rect::default()));
        // Drop window 2 on the left part of window 1.
        let r1 = wm.desired_rect(1).unwrap();
        wm.on_user_move(2, Rect::new(0, 0, 10, 10), (r1.x as f64 + 10.0, r1.center().1));
        assert_eq!(wm.workspaces[&1].layout.windows(), vec![2, 1]);
    }

    #[test]
    fn master_layout_via_config() {
        let mut wm = Wm::new(WmConfig { layout: LayoutKind::Master, ..Default::default() }, vec![mon("A", 0)]);
        for i in 1..=3 {
            wm.add_window(i, tiled(Rect::default()));
        }
        wm.layout_msg("swapwithmaster");
        let rects = wm.tiled_rects(1);
        assert_eq!(rects[0].0, 3);
    }

    #[test]
    fn learned_min_size_retiles_without_overlap() {
        let mut wm = wm1();
        for i in 1..=3 {
            wm.add_window(i, tiled(Rect::default()));
        }
        wm.set_min_size(3, 1200, 0);
        let rects = wm.tiled_rects(1);
        let r3 = rects.iter().find(|x| x.0 == 3).unwrap().1;
        assert!(r3.w >= 1200, "{r3:?}");
        for (a, ra) in &rects {
            for (b, rb) in &rects {
                if a < b {
                    let ow = ra.right().min(rb.right()) - ra.x.max(rb.x);
                    let oh = ra.bottom().min(rb.bottom()) - ra.y.max(rb.y);
                    assert!(ow <= 0 || oh <= 0, "{a} and {b} overlap");
                }
            }
        }
    }

    #[test]
    fn auto_floated_window_retiles_when_moved_or_when_room_frees_up() {
        let mut wm = wm1();
        for i in 1..=3 {
            wm.add_window(i, tiled(Rect::default()));
        }
        // Layout: 1 | (2 over 3). The right column is 1040px tall; 2 and 3
        // together need 400 + 800 (+ gaps), so 3 cannot fit beside 2.
        wm.set_min_size(2, 0, 400);
        wm.set_min_size(3, 0, 800);
        wm.auto_float(3);
        assert!(wm.windows[&3].floating && wm.windows[&3].auto_float);
        // Moving it to an empty workspace tiles it there.
        wm.move_to_workspace(Some(3), &WsSel::Id(2), false);
        assert!(!wm.windows[&3].floating, "tiles on the new workspace");
        assert!(!wm.windows[&3].auto_float);
        // Back on ws 1 it does not fit, so it floats again (still automatic).
        wm.move_to_workspace(Some(3), &WsSel::Id(1), false);
        assert!(wm.windows[&3].floating && wm.windows[&3].auto_float);
        // Closing window 2 frees enough room: it tiles by itself.
        wm.remove_window(2);
        assert!(!wm.windows[&3].floating);
        assert!(wm.tiled_rects(1).iter().find(|x| x.0 == 3).unwrap().1.h >= 800);
    }

    #[test]
    fn overflow_moves_to_next_workspace_that_fits() {
        let mut wm = wm1();
        for i in 1..=3 {
            wm.add_window(i, tiled(Rect::default()));
        }
        // Workspace 2 already has a wide window; 3 cannot fit beside it either.
        wm.move_to_workspace(Some(1), &WsSel::Id(2), false);
        wm.set_min_size(1, 1500, 0);
        wm.set_min_size(3, 1500, 0);
        let (_, to) = wm.overflow(3);
        assert_eq!(to, Some(3), "skips full ws 2, creates ws 3");
        assert_eq!(wm.windows[&3].ws, 3);
        assert!(!wm.windows[&3].floating);
        assert_eq!(wm.active_workspace(), 1, "user is not moved");
        assert!(!wm.windows[&3].shown, "hidden on the inactive workspace");
    }

    #[test]
    fn overflow_ws_evicts_the_newest_window_not_the_squeezed_one() {
        let mut wm = wm1();
        for i in 1..=3 {
            wm.add_window(i, tiled(Rect::default()));
        }
        // Layout is 1 | (2 over 3) in an 1880px-wide area. 1 and 2 (900px each)
        // fit side by side; 3 needs 1000px in the right column, which does not.
        wm.set_min_size(1, 900, 0);
        wm.set_min_size(2, 900, 0);
        wm.set_min_size(3, 1000, 0);
        let (_, moved) = wm.overflow_ws(1);
        assert_eq!(moved, vec![(3, Some(2))]);
        assert_eq!(wm.windows[&1].ws, 1);
        assert_eq!(wm.windows[&2].ws, 1);
    }

    #[test]
    fn overflow_uses_existing_workspace_with_room() {
        let mut wm = wm1();
        wm.add_window(1, tiled(Rect::default()));
        wm.add_window(2, tiled(Rect::default()));
        wm.add_window(3, tiled(Rect::default()));
        wm.move_to_workspace(Some(3), &WsSel::Id(2), false);
        wm.set_min_size(2, 1200, 0);
        let (_, to) = wm.overflow(2);
        assert_eq!(to, Some(2));
        assert_eq!(wm.workspaces[&2].layout_windows().len(), 2);
    }

    #[test]
    fn overflow_floats_when_it_fits_nowhere() {
        let mut wm = wm1();
        wm.add_window(1, tiled(Rect::default()));
        wm.add_window(2, tiled(Rect::default()));
        wm.set_min_size(2, 5000, 0);
        let (_, to) = wm.overflow(2);
        assert_eq!(to, None);
        assert!(wm.windows[&2].floating && wm.windows[&2].auto_float);
        assert_eq!(wm.windows[&2].ws, 1);
        assert!(!wm.workspaces.contains_key(&2), "trial workspace cleaned up");
    }

    #[test]
    fn user_float_choice_is_never_undone() {
        let mut wm = wm1();
        wm.add_window(1, tiled(Rect::default()));
        wm.add_window(2, tiled(Rect::default()));
        wm.set_floating(Some(2), Some(true));
        wm.move_to_workspace(Some(2), &WsSel::Id(3), false);
        assert!(wm.windows[&2].floating);
    }

    #[test]
    fn pseudotile_centres_natural_size() {
        let mut wm = wm1();
        wm.add_window(1, NewWindow { rect: Rect::new(0, 0, 400, 300), pseudo: true, ..Default::default() });
        let r = wm.desired_rect(1).unwrap();
        assert_eq!((r.w, r.h), (400, 300));
        assert_eq!(r.center(), (960.0, 540.0));
    }

    #[test]
    fn scaled_monitor_scales_gaps() {
        let mut wm = Wm::new(WmConfig::default(), vec![MonitorSpec { scale: 1.5, ..mon("A", 0) }]);
        wm.add_window(1, tiled(Rect::default()));
        assert_eq!(wm.desired_rect(1), Some(Rect::new(30, 30, 1860, 1020)));
    }
}
