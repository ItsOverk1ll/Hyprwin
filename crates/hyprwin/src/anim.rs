//! The animation engine: a short-lived thread that draws live DWM thumbnails
//! of animated windows into one click-through overlay, paced by the
//! compositor clock (the monitor's real refresh rate).
//!
//! The main thread moves each real window once (cloaked, so it isn't seen
//! twice) and sends the thread what to draw. When every item has arrived, the
//! thread posts [`WM_APP_ANIM_DONE`]; the main thread uncloaks the real windows
//! *under* the still-visible overlay and calls [`Animator::release`], after
//! which the overlay disappears. New changes mid-flight retarget items from
//! where they are drawn now, as Hyprland does.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use hyprwin_anim::{Bezier, Frame, lerp};
use hyprwin_layout::{Rect, WinId};
use hyprwin_win32::ghost::{LayerMode, Overlay};
use hyprwin_win32::{HWND, clock::FrameClock, dwm, window};
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, PostMessageW, TranslateMessage, WM_APP};

use crate::debug;

pub const WM_APP_ANIM_DONE: u32 = WM_APP + 30;

/// How long the thread keeps the final frame up waiting for the main thread.
const RELEASE_TIMEOUT: Duration = Duration::from_millis(600);

#[derive(Clone, Copy, Debug)]
pub struct Item {
    pub id: WinId,
    /// Where to start. `None` = from wherever it is drawn now (or its real frame).
    pub from: Option<Frame>,
    pub to: Frame,
    pub curve: Bezier,
    pub duration: Duration,
}

enum Msg {
    Update(u64, Vec<Item>),
    Release(u64),
}

struct Running {
    tx: Sender<Msg>,
}

pub struct Animator {
    main: HWND,
    seq: u64,
    running: Option<Running>,
    /// Frames currently on screen, shared with the thread (for retargeting).
    drawn: Arc<Mutex<HashMap<WinId, Frame>>>,
}

fn frame_of(r: Rect) -> Frame {
    Frame { rect: r, opacity: 1.0 }
}

impl Animator {
    pub fn new(main: HWND) -> Self {
        Animator { main, seq: 0, running: None, drawn: Arc::new(Mutex::new(HashMap::new())) }
    }

    pub fn generation(&self) -> u64 {
        self.seq
    }

    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    /// The frame `id` is drawn at right now, if it is animating.
    pub fn drawn(&self, id: WinId) -> Option<Frame> {
        if self.running.is_none() {
            return None;
        }
        self.drawn.lock().ok()?.get(&id).copied()
    }

    /// Starts or retargets items. `bounds` covers every monitor.
    pub fn animate(&mut self, items: Vec<Item>, bounds: Rect) {
        if items.is_empty() {
            return;
        }
        self.seq += 1;
        if let Some(r) = &self.running {
            if r.tx.send(Msg::Update(self.seq, items.clone())).is_ok() {
                return;
            }
            self.running = None; // thread died; start a fresh one
        }
        let (tx, rx) = channel();
        let _ = tx.send(Msg::Update(self.seq, items));
        // Each thread gets its own map, so an old thread winding down can't
        // disturb a new one.
        self.drawn = Arc::new(Mutex::new(HashMap::new()));
        let drawn = self.drawn.clone();
        let main = self.main.0 as isize;
        let spawned = std::thread::Builder::new().name("hyprwin-anim".into()).spawn(move || {
            run(rx, drawn, HWND(main as *mut _), bounds);
        });
        if spawned.is_ok() {
            self.running = Some(Running { tx });
        }
    }

    /// Called after the main thread finished uncloaking for `seq`: the overlay
    /// may go now. Returns false if `seq` is stale (a newer update is running).
    pub fn release(&mut self, seq: u64) -> bool {
        if seq != self.seq {
            return false;
        }
        if let Some(r) = self.running.take() {
            let _ = r.tx.send(Msg::Release(seq));
        }
        true
    }

    /// Stops immediately (pause, exit).
    pub fn cancel(&mut self) {
        if let Some(r) = self.running.take() {
            let _ = r.tx.send(Msg::Release(self.seq));
        }
    }
}

struct Live {
    id: WinId,
    thumb: usize,
    from: Frame,
    to: Frame,
    start: Instant,
    curve: Bezier,
    duration: Duration,
}

fn lerp_frame(a: Frame, b: Frame, t: f64) -> Frame {
    Frame { rect: Rect::lerp(a.rect, b.rect, t), opacity: lerp(a.opacity, b.opacity, t).clamp(0.0, 1.0) }
}

/// The part of the source window to show: its visible frame, without the
/// invisible resize borders and shadow (window-relative coordinates).
fn crop(h: HWND) -> Option<Rect> {
    let wr = window::window_rect(h).ok()?;
    let fb = dwm::frame_bounds(h).ok()?;
    Some(Rect::new(fb.x - wr.x, fb.y - wr.y, fb.w, fb.h))
}

fn pump() {
    let mut msg = MSG::default();
    unsafe {
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

fn run(rx: Receiver<Msg>, drawn: Arc<Mutex<HashMap<WinId, Frame>>>, main: HWND, bounds: Rect) {
    let mut overlay = match Overlay::new(bounds, LayerMode::ColorKey) {
        Ok(o) => o,
        Err(e) => {
            debug!("animation overlay failed: {e}");
            let _ = unsafe { PostMessageW(Some(main), WM_APP_ANIM_DONE, WPARAM(0), LPARAM(0)) };
            return;
        }
    };
    let mut clock = FrameClock::new();
    let mut items: Vec<Live> = Vec::new();
    let mut seq = 0u64;
    let mut waiting: Option<Instant> = None;
    let mut frames = 0u32;
    'outer: loop {
        loop {
            match rx.try_recv() {
                Ok(Msg::Update(g, list)) => {
                    seq = g;
                    waiting = None;
                    let now = Instant::now();
                    for it in list {
                        let h = HWND(it.id as *mut _);
                        let current = drawn.lock().ok().and_then(|d| d.get(&it.id).copied());
                        let from = it.from.or(current).unwrap_or_else(|| frame_of(dwm::frame_bounds(h).unwrap_or(it.to.rect)));
                        if let Some(l) = items.iter_mut().find(|l| l.id == it.id) {
                            l.from = current.unwrap_or(from);
                            l.to = it.to;
                            l.start = now;
                            l.curve = it.curve;
                            l.duration = it.duration;
                        } else if let Ok(thumb) = overlay.add(h, crop(h)) {
                            let _ = overlay.set_cropped(thumb, from.rect, crop(h), (from.opacity * 255.0) as u8);
                            items.push(Live { id: it.id, thumb, from, to: it.to, start: now, curve: it.curve, duration: it.duration });
                        }
                    }
                }
                Ok(Msg::Release(g)) if g == seq => break 'outer,
                Ok(Msg::Release(_)) => {}
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break 'outer,
            }
        }
        pump();
        if let Some(since) = waiting {
            if since.elapsed() > RELEASE_TIMEOUT {
                break;
            }
        } else {
            let now = Instant::now();
            let mut all_done = true;
            let mut out = Vec::with_capacity(items.len());
            for l in &items {
                let p = if l.duration.is_zero() { 1.0 } else { (now - l.start).as_secs_f64() / l.duration.as_secs_f64() };
                let done = p >= 1.0;
                all_done &= done;
                let f = if done { l.to } else { lerp_frame(l.from, l.to, l.curve.ease(p.clamp(0.0, 1.0))) };
                let h = HWND(l.id as *mut _);
                let _ = overlay.set_cropped(l.thumb, f.rect, crop(h), (f.opacity * 255.0).round() as u8);
                out.push((l.id, f));
            }
            if let Ok(mut d) = drawn.lock() {
                d.extend(out);
            }
            frames += 1;
            if all_done {
                debug!(
                    "anim thread: done after {} frames; items: {}",
                    frames,
                    items.iter().map(|l| format!("{:#x} {}ms/{}ms", l.id, (now - l.start).as_millis(), l.duration.as_millis())).collect::<Vec<_>>().join(", ")
                );
                waiting = Some(Instant::now());
                let _ = unsafe { PostMessageW(Some(main), WM_APP_ANIM_DONE, WPARAM(seq as usize), LPARAM(0)) };
            }
        }
        clock.wait();
    }
    // The main thread has uncloaked the real windows under the overlay:
    // let one composition pass show them, then remove the overlay.
    clock.wait();
    drop(overlay);
    debug!("animation finished after {frames} frames");
}
