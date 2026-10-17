//! Turning layout effects into animations (see `crate::anim` for the engine).
//!
//! Before a batch of effects runs, [`App::plan_animations`] decides what to
//! animate and cloaks those windows (recorded in the ledger first, like any
//! hide). The batch then moves the real windows once. When the engine has
//! drawn the last frame, [`App::on_anim_done`] uncloaks them under the overlay.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use hyprwin_anim::{Frame, WindowStyle, WorkspaceStyle, window_frame, workspace_frame};
use hyprwin_config::ResolvedAnim;
use hyprwin_layout::wm::{Anim, Effect};
use hyprwin_layout::{MonId, Rect, WinId, WsId};
use hyprwin_win32::ledger::HideMethod;
use hyprwin_win32::{cloak, dwm, window};
use windows::Win32::UI::WindowsAndMessaging::IsIconic;

use super::{App, hwnd};
use crate::anim::Item;
use crate::debug;

/// What the main thread still owes a window when its animation ends.
#[derive(Clone, Debug, Default)]
pub struct Pending {
    /// Cloaked only for the animation: uncloak at the end.
    pub cloaked: bool,
    /// Arriving with a workspace switch: reveal at the end.
    pub reveal: bool,
}

/// How the current batch must deviate from running every effect right away.
#[derive(Default)]
pub struct Plan {
    pub items: Vec<Item>,
    /// Shows to hold back until the animation ends.
    pub defer_show: HashSet<WinId>,
}

fn frame_of(r: Rect) -> Frame {
    frame(r)
}

fn frame(r: Rect) -> Frame {
    Frame { rect: r, opacity: 1.0 }
}

fn near(a: Rect, b: Rect) -> bool {
    (a.x - b.x).abs() <= 2 && (a.y - b.y).abs() <= 2 && (a.w - b.w).abs() <= 2 && (a.h - b.h).abs() <= 2
}

fn item(id: WinId, from: Frame, to: Frame, a: &ResolvedAnim) -> Item {
    Item { id, from: Some(from), to, curve: a.curve, duration: Duration::from_secs_f64(a.duration_ms / 1000.0), cloak_source: false }
}

impl App {
    fn monitor_rect_of(&self, id: WinId) -> Option<Rect> {
        let ws = self.wm.windows.get(&id)?.ws;
        let m = self.wm.workspaces.get(&ws)?.monitor;
        Some(self.wm.monitors.get(m)?.spec.rect)
    }

    /// Union of all monitors: the overlay covers them all, so windows can
    /// animate across monitors.
    fn screen_bounds(&self) -> Rect {
        let mut it = self.wm.monitors.iter().map(|m| m.spec.rect);
        let Some(first) = it.next() else { return Rect::new(0, 0, 1, 1) };
        it.fold(first, |a, r| {
            let x = a.x.min(r.x);
            let y = a.y.min(r.y);
            Rect::new(x, y, a.right().max(r.right()) - x, a.bottom().max(r.bottom()) - y)
        })
    }

    /// Where the window is on screen now: mid-animation, its drawn frame.
    fn shown_frame(&self, id: WinId) -> Option<Frame> {
        self.animator.drawn(id).or_else(|| dwm::frame_bounds(hwnd(id)).ok().map(frame))
    }

    /// True if the window is hidden for a workspace (not just for an animation).
    fn hidden_for_workspace(&self, id: WinId) -> bool {
        self.ledger.hidden.contains_key(&(hwnd(id).0 as isize)) && !self.anim_pending.get(&id).is_some_and(|p| p.cloaked)
    }

    fn active_by_monitor(&self) -> HashMap<MonId, WsId> {
        self.wm.monitors.iter().enumerate().map(|(i, m)| (i, m.active)).collect()
    }

    pub fn plan_animations(&mut self, fx: &[Effect]) -> Plan {
        let now_active = self.active_by_monitor();
        let before = std::mem::replace(&mut self.anim_active_ws, now_active.clone());
        let mut plan = Plan::default();
        if self.anim_quiet {
            // Still remember the targets, so the next real change animates
            // from the right place and a re-sent target stays a correction.
            for e in fx {
                if let Effect::Place { id, rect, .. } = *e {
                    self.anim_target.insert(id, rect);
                }
            }
            return plan;
        }
        if self.paused || self.drag.is_some() || !self.cfg.animations_enabled {
            return plan;
        }
        // Monitors whose workspace switched in this batch: (old, new).
        let switched: HashMap<MonId, (WsId, WsId)> =
            now_active.iter().filter_map(|(m, &new)| before.get(m).filter(|&&old| old != new).map(|&old| (*m, (old, new)))).collect();
        let ws_mon = |app: &App, id: WinId| -> Option<(MonId, WsId)> {
            let ws = app.wm.windows.get(&id)?.ws;
            Some((app.wm.workspaces.get(&ws)?.monitor, ws))
        };
        let move_a = self.cfg.anim("windowsMove");
        let open_a = self.cfg.anim("windowsIn");
        let in_a = self.cfg.anim("workspacesIn");
        let out_a = self.cfg.anim("workspacesOut");
        let mut to_cloak: Vec<WinId> = Vec::new();

        for e in fx {
            match *e {
                Effect::Place { id, rect, anim } => {
                    let a = match anim {
                        Anim::Open => open_a.as_ref(),
                        Anim::Move => move_a.as_ref(),
                        Anim::None => None,
                    };
                    let Some(a) = a else { continue };
                    let h = hwnd(id);
                    let blocked = !window::is_alive(h)
                        || unsafe { IsIconic(h).as_bool() }
                        || self.stubborn.contains(&id)
                        || self.in_movesize.contains(&id)
                        || self.hidden_for_workspace(id)
                        || self.wm.windows.get(&id).is_none_or(|w| w.minimized);
                    // Re-sending the same target is a correction, not a change.
                    let same_target = self.anim_target.insert(id, rect) == Some(rect);
                    if blocked || same_target {
                        continue;
                    }
                    let Some(mon) = self.monitor_rect_of(id) else { continue };
                    let from = if anim == Anim::Open && self.animator.drawn(id).is_none() {
                        let style = a.style.as_deref().and_then(WindowStyle::parse).unwrap_or(WindowStyle::Popin { start_percent: 80.0 });
                        window_frame(style, rect, mon, 0.0, false)
                    } else {
                        let Some(f) = self.shown_frame(id) else { continue };
                        if near(f.rect, rect) && f.opacity >= 1.0 {
                            continue;
                        }
                        f
                    };
                    plan.items.push(item(id, from, frame(rect), a));
                    if !self.anim_pending.get(&id).is_some_and(|p| p.cloaked) {
                        to_cloak.push(id);
                    }
                }
                Effect::Hide(id) => {
                    let Some(a) = out_a.as_ref() else { continue };
                    let Some((m, ws)) = ws_mon(self, id) else { continue };
                    let Some(&(old, new)) = switched.get(&m) else { continue };
                    if ws != old || self.hidden_for_workspace(id) || self.wm.windows.get(&id).is_none_or(|w| w.minimized) {
                        continue;
                    }
                    let Some(from) = self.shown_frame(id) else { continue };
                    let mon = self.wm.monitors[m].spec.rect;
                    let style = a.style.as_deref().and_then(WorkspaceStyle::parse).unwrap_or(WorkspaceStyle::Slide);
                    let rest = self.anim_target.get(&id).copied().unwrap_or(from.rect);
                    let to = workspace_frame(style, rest, mon, 1.0, false, new > old);
                    plan.items.push(item(id, from, to, a));
                    // From now on it is a workspace hide; the end of the
                    // animation must not uncloak it.
                    self.anim_pending.remove(&id);
                }
                Effect::Show(id) => {
                    let Some(a) = in_a.as_ref() else { continue };
                    let Some((m, ws)) = ws_mon(self, id) else { continue };
                    let Some(&(old, new)) = switched.get(&m) else { continue };
                    let Some(rest) = self.wm.desired_rect(id) else { continue };
                    if ws != new || !self.hidden_for_workspace(id) {
                        continue;
                    }
                    let mon = self.wm.monitors[m].spec.rect;
                    let style = a.style.as_deref().and_then(WorkspaceStyle::parse).unwrap_or(WorkspaceStyle::Slide);
                    let from = workspace_frame(style, rest, mon, 0.0, true, new > old);
                    plan.items.push(item(id, from, frame(rest), a));
                    self.anim_target.insert(id, rest);
                    plan.defer_show.insert(id);
                    self.anim_pending.entry(id).or_default().reveal = true;
                }
                _ => {}
            }
        }

        // Cloak first (recorded before acting, saved once), then the batch moves them.
        if !to_cloak.is_empty() {
            for &id in &to_cloak {
                self.ledger.hidden.entry(hwnd(id).0 as isize).or_insert(HideMethod::Cloak);
            }
            let _ = self.ledger.save();
            for id in to_cloak {
                if cloak::set_cloaked(hwnd(id), true).is_ok() {
                    self.anim_pending.entry(id).or_default().cloaked = true;
                } else {
                    self.ledger.hidden.remove(&(hwnd(id).0 as isize));
                    plan.items.retain(|i| i.id != id);
                }
            }
        }
        plan
    }

    pub fn start_animations(&mut self, plan: Plan) {
        if plan.items.is_empty() {
            return;
        }
        debug!("animating {} window(s) for up to {} ms", plan.items.len(), plan.items.iter().map(|i| i.duration.as_millis()).max().unwrap_or(0));
        let bounds = self.screen_bounds();
        self.anim_started = Some(Instant::now());
        self.animator.animate(plan.items, bounds);
    }

    /// The engine drew the last frame of generation `seq`.
    pub fn on_anim_done(&mut self, seq: u64) {
        if seq != self.animator.generation() {
            return; // a newer update is still animating
        }
        let pending = std::mem::take(&mut self.anim_pending);
        for (id, p) in pending {
            let visible = self.wm.windows.get(&id).is_some_and(|w| !w.minimized) && self.wm.desired_rect(id).is_some();
            let key = hwnd(id).0 as isize;
            if p.cloaked && visible && matches!(self.ledger.hidden.get(&key), Some(HideMethod::Cloak)) {
                self.ledger.hidden.remove(&key);
                let _ = cloak::set_cloaked(hwnd(id), false);
            }
            if p.reveal && visible {
                self.reveal(id);
            }
        }
        let _ = self.ledger.save();
        self.animator.release(seq);
        // Closing pictures have played out (their last frame is invisible).
        for snap in std::mem::take(&mut self.close_snaps) {
            crate::snapshot::destroy(snap);
        }
        if let Some(f) = self.anim_focus.take() {
            if self.wm.focus == Some(f) && self.wm.desired_rect(f).is_some() {
                self.apply(vec![Effect::Focus(f)]);
            }
        }
        if let Some(t) = self.anim_started.take() {
            debug!("animation done in {} ms", t.elapsed().as_millis());
        }
        self.schedule_verify();
        self.sync_border();
    }

    /// Stops animating and puts every window in its final state (pause, exit, reload).
    pub fn finish_animations_now(&mut self) {
        let seq = self.animator.generation();
        if self.animator.is_running() || !self.anim_pending.is_empty() {
            self.on_anim_done(seq);
        }
        self.animator.cancel();
    }

    // ------------------------------------------------------------------ closing

    /// Closes a window hyprwin was asked to close (`killactive`,
    /// `closewindow`), first putting a picture of it in its place so its
    /// `windowsOut` animation can play once it is gone.
    pub fn close_window(&mut self, id: WinId) {
        use windows::Win32::Foundation::{LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, SetTimer, WM_CLOSE};
        let h = hwnd(id);
        let animate = !self.anim_quiet
            && !self.paused
            && self.cfg.anim("windowsOut").is_some()
            && self.wm.desired_rect(id).is_some()
            && !self.hidden_for_workspace(id)
            && window::is_visible(h)
            && !unsafe { IsIconic(h).as_bool() };
        if animate {
            let fullscreen = self.wm.fullscreen_of(id).is_some();
            let radius = if fullscreen { 0 } else { crate::border::corner_radius(self.cfg.rounding, crate::border::scale_of(h)) };
            if let Some(pic) = crate::snapshot::capture(h, radius) {
                if let Some(snap) = crate::snapshot::show(&pic, h) {
                    if let Some((old, _, _)) = self.closing.insert(id, (snap, pic.frame, Instant::now())) {
                        crate::snapshot::destroy(old);
                    }
                    unsafe {
                        SetTimer(Some(self.main), super::TIMER_CLOSE, 400, None);
                    }
                }
            }
        }
        unsafe {
            let _ = PostMessageW(Some(h), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }

    /// The window being closed disappeared: its picture plays `windowsOut`.
    pub fn on_closed(&mut self, id: WinId) {
        let Some((snap, frame, _)) = self.closing.remove(&id) else { return };
        let Some(a) = self.cfg.anim("windowsOut") else {
            crate::snapshot::destroy(snap);
            return;
        };
        let (cx, cy) = frame.center();
        let mon = self
            .wm
            .monitors
            .iter()
            .map(|m| m.spec.rect)
            .find(|m| cx >= m.x as f64 && cx < m.right() as f64 && cy >= m.y as f64 && cy < m.bottom() as f64)
            .unwrap_or(frame);
        let style = a.style.as_deref().and_then(WindowStyle::parse).unwrap_or(WindowStyle::Popin { start_percent: 87.0 });
        let to = window_frame(style, frame, mon, 1.0, true);
        let mut it = item(snap.0 as isize as WinId, frame_of(frame), to, &a);
        it.cloak_source = true;
        self.close_snaps.push(snap);
        self.start_animations(Plan { items: vec![it], ..Default::default() });
    }

    /// Windows that did not close after all (an app asking to save changes)
    /// get their picture removed.
    pub fn close_timeout(&mut self) {
        let stale: Vec<WinId> = self.closing.iter().filter(|(_, (_, _, t))| t.elapsed() >= Duration::from_millis(350)).map(|(id, _)| *id).collect();
        for id in stale {
            if let Some((snap, _, _)) = self.closing.remove(&id) {
                crate::snapshot::destroy(snap);
            }
        }
        if self.closing.is_empty() {
            unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::KillTimer(Some(self.main), super::TIMER_CLOSE);
            }
        }
    }

    pub fn is_animating(&self, id: WinId) -> bool {
        self.anim_pending.contains_key(&id)
    }
}
