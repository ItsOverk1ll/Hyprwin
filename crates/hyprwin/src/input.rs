//! Low-level keyboard and mouse hooks on a dedicated high-priority thread.
//!
//! The callbacks must be fast: Windows silently removes an LL hook whose
//! callback exceeds `LowLevelHooksTimeout`. They only consult an immutable bind
//! table and `PostMessage` the main thread. No allocation on the hot path
//! except when a bind actually fires.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

use hyprwin_config::{Bind, BindFlags, Cmd, Key};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{GetCurrentThread, GetCurrentThreadId, SetThreadPriority, THREAD_PRIORITY_TIME_CRITICAL};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, MAPVK_VSC_TO_VK_EX, MapVirtualKeyW, SendInput, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT, PostMessageW, SetWindowsHookExW, TranslateMessage,
    WH_KEYBOARD_LL, WH_MOUSE_LL, WM_APP, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL,
    WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN, WM_XBUTTONDOWN, WM_XBUTTONUP,
};

pub const WM_APP_BIND: u32 = WM_APP + 1;
pub const WM_APP_MOUSE: u32 = WM_APP + 2;
pub const WM_APP_DRAG: u32 = WM_APP + 3;

pub const DRAG_MOVE: usize = 0;
pub const DRAG_RESIZE: usize = 1;
pub const DRAG_END: usize = 2;

/// `dwExtraInfo` on input we inject ourselves ("HYPR").
pub const MARK: usize = 0x4859_5052;
const VK_MASK: u16 = 0xE8; // unassigned: used to stop Win/Alt taps opening menus

const M_SUPER: u8 = 1;
const M_SHIFT: u8 = 2;
const M_CTRL: u8 = 4;
const M_ALT: u8 = 8;

#[derive(Clone, Debug)]
struct HookBind {
    mods: u8,
    flags: BindFlags,
    submap: u32,
    /// Index into the daemon's bind list.
    idx: usize,
    drag: Option<usize>,
}

#[derive(Default)]
struct Table {
    by_key: HashMap<Key, Vec<HookBind>>,
}

pub struct Shared {
    table: RwLock<Arc<Table>>,
    pub submap: AtomicU32,
    pub main: AtomicIsize,
    pub follow_mouse: AtomicBool,
    mouse_pending: AtomicBool,
    last_mouse: AtomicU64,
    /// 0 = none, 1 = left, 2 = right
    drag_button: AtomicU8,
    pub thread_id: AtomicU32,
}

pub fn shared() -> &'static Shared {
    static S: OnceLock<Shared> = OnceLock::new();
    S.get_or_init(|| Shared {
        table: RwLock::new(Arc::new(Table::default())),
        submap: AtomicU32::new(0),
        main: AtomicIsize::new(0),
        follow_mouse: AtomicBool::new(true),
        mouse_pending: AtomicBool::new(false),
        last_mouse: AtomicU64::new(0),
        drag_button: AtomicU8::new(0),
        thread_id: AtomicU32::new(0),
    })
}

pub fn pack(x: i32, y: i32) -> u64 {
    ((y as u32 as u64) << 32) | (x as u32 as u64)
}

pub fn unpack(v: u64) -> (i32, i32) {
    (v as u32 as i32, (v >> 32) as u32 as i32)
}

pub fn last_mouse() -> (i32, i32) {
    shared().mouse_pending.store(false, Ordering::Release);
    unpack(shared().last_mouse.load(Ordering::Acquire))
}

/// Rebuilds the hook's bind table. `submaps` maps names to ids (0 = global).
pub fn set_binds(binds: &[Bind], submaps: &HashMap<String, u32>) {
    let mut t = Table::default();
    for (idx, b) in binds.iter().enumerate() {
        let key = match b.key {
            Key::Scan(sc) => {
                let vk = unsafe { MapVirtualKeyW(sc as u32, MAPVK_VSC_TO_VK_EX) } as u16;
                if vk == 0 {
                    continue;
                }
                Key::Vk(vk)
            }
            k => k,
        };
        let drag = match (&b.cmd, b.flags.mouse) {
            (Cmd::MouseMove, true) => Some(DRAG_MOVE),
            (Cmd::MouseResize, true) => Some(DRAG_RESIZE),
            _ => None,
        };
        let submap = b.submap.as_ref().and_then(|s| submaps.get(s).copied()).unwrap_or(0);
        t.by_key.entry(key).or_default().push(HookBind { mods: b.mods.0, flags: b.flags, submap, idx, drag });
    }
    *shared().table.write().unwrap() = Arc::new(t);
}

struct HookState {
    down: [bool; 256],
    swallowed: [bool; 256],
    pending_release: [Option<usize>; 256],
}

impl Default for HookState {
    fn default() -> Self {
        Self { down: [false; 256], swallowed: [false; 256], pending_release: [None; 256] }
    }
}

impl HookState {
    fn mods(&self) -> u8 {
        let d = |v: usize| self.down[v];
        let mut m = 0;
        if d(0x5B) || d(0x5C) {
            m |= M_SUPER;
        }
        if d(0xA0) || d(0xA1) || d(0x10) {
            m |= M_SHIFT;
        }
        if d(0xA2) || d(0xA3) || d(0x11) {
            m |= M_CTRL;
        }
        if d(0xA4) || d(0xA5) || d(0x12) {
            m |= M_ALT;
        }
        m
    }
}

thread_local! {
    static STATE: RefCell<HookState> = RefCell::new(HookState::default());
}

fn is_modifier(vk: usize) -> bool {
    matches!(vk, 0x10 | 0x11 | 0x12 | 0x5B | 0x5C | 0xA0..=0xA5)
}

fn post(msg: u32, w: usize, l: u64) {
    let main = HWND(shared().main.load(Ordering::Relaxed) as *mut _);
    unsafe {
        let _ = PostMessageW(Some(main), msg, WPARAM(w), LPARAM(l as isize));
    }
}

/// Injects a no-op key so the shell never sees a "bare" Win or Alt tap.
fn mask() {
    let ev = |up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(VK_MASK),
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                time: 0,
                dwExtraInfo: MARK,
            },
        },
    };
    unsafe {
        SendInput(&[ev(false), ev(true)], std::mem::size_of::<INPUT>() as i32);
    }
}

fn matches_for(key: Key, mods: u8, mouse: bool) -> Vec<HookBind> {
    let t = shared().table.read().unwrap().clone();
    let sub = shared().submap.load(Ordering::Relaxed);
    t.by_key
        .get(&key)
        .map(|v| {
            v.iter()
                .filter(|b| b.submap == sub && (b.mods == mods || b.flags.ignore_mods) && (b.flags.mouse == mouse || !mouse))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// Ctrl+Alt+Shift+Win+Esc always passes through to `hyprwin-guard`.
fn is_emergency(vk: usize, mods: u8) -> bool {
    vk == 0x1B && mods == (M_SUPER | M_SHIFT | M_CTRL | M_ALT)
}

unsafe extern "system" fn kb_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code >= 0 {
        let k = unsafe { &*(lp.0 as *const KBDLLHOOKSTRUCT) };
        if k.dwExtraInfo != MARK {
            let down = wp.0 as u32 == WM_KEYDOWN || wp.0 as u32 == WM_SYSKEYDOWN;
            if handle_key((k.vkCode & 0xFF) as usize, down) {
                return LRESULT(1);
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wp, lp) }
}

fn handle_key(vk: usize, down: bool) -> bool {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        if down {
            let repeat = s.down[vk];
            s.down[vk] = true;
            if is_modifier(vk) {
                return false;
            }
            let mods = s.mods();
            if is_emergency(vk, mods) {
                return false;
            }
            let hits = matches_for(Key::Vk(vk as u16), mods, false);
            if hits.is_empty() {
                return s.swallowed[vk] && repeat;
            }
            let mut consume = false;
            for b in &hits {
                if b.flags.release {
                    s.pending_release[vk] = Some(b.idx);
                } else if !repeat || b.flags.repeat {
                    post(WM_APP_BIND, b.idx, 0);
                }
                consume |= !b.flags.non_consuming;
            }
            if consume {
                s.swallowed[vk] = true;
                if !repeat && mods & (M_SUPER | M_ALT) != 0 {
                    mask();
                }
            }
            consume
        } else {
            s.down[vk] = false;
            if let Some(idx) = s.pending_release[vk].take() {
                post(WM_APP_BIND, idx, 0);
            }
            std::mem::take(&mut s.swallowed[vk])
        }
    })
}

unsafe extern "system" fn mouse_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code >= 0 {
        let m = unsafe { &*(lp.0 as *const MSLLHOOKSTRUCT) };
        if m.dwExtraInfo != MARK && handle_mouse(wp.0 as u32, m) {
            return LRESULT(1);
        }
    }
    unsafe { CallNextHookEx(None, code, wp, lp) }
}

fn handle_mouse(msg: u32, m: &MSLLHOOKSTRUCT) -> bool {
    let sh = shared();
    let pos = pack(m.pt.x, m.pt.y);
    if msg == WM_MOUSEMOVE {
        sh.last_mouse.store(pos, Ordering::Release);
        if (sh.follow_mouse.load(Ordering::Relaxed) || sh.drag_button.load(Ordering::Relaxed) != 0) && !sh.mouse_pending.swap(true, Ordering::AcqRel) {
            post(WM_APP_MOUSE, 0, pos);
        }
        return false;
    }
    // Ending a drag: swallow the matching button-up.
    let dragging = sh.drag_button.load(Ordering::Relaxed);
    if (msg == WM_LBUTTONUP && dragging == 1) || (msg == WM_RBUTTONUP && dragging == 2) {
        sh.drag_button.store(0, Ordering::Relaxed);
        post(WM_APP_DRAG, DRAG_END, pos);
        return true;
    }
    let mods = STATE.with(|s| s.borrow().mods());
    let key = match msg {
        WM_LBUTTONDOWN => Key::MouseLeft,
        WM_RBUTTONDOWN => Key::MouseRight,
        WM_MBUTTONDOWN => Key::MouseMiddle,
        WM_XBUTTONDOWN => {
            if (m.mouseData >> 16) & 0xFFFF == 1 { Key::MouseX1 } else { Key::MouseX2 }
        }
        WM_MOUSEWHEEL => {
            let delta = ((m.mouseData >> 16) & 0xFFFF) as u16 as i16;
            if delta > 0 { Key::WheelUp } else { Key::WheelDown }
        }
        WM_MBUTTONUP | WM_XBUTTONUP => return false,
        _ => return false,
    };
    if mods == 0 && matches!(key, Key::MouseLeft | Key::MouseRight) {
        return false; // fast path: plain clicks are never bound
    }
    let hits = matches_for(key, mods, true);
    let hits: Vec<HookBind> = if hits.is_empty() { matches_for(key, mods, false) } else { hits };
    let Some(b) = hits.first() else { return false };
    if let Some(kind) = b.drag {
        sh.drag_button.store(if key == Key::MouseLeft { 1 } else { 2 }, Ordering::Relaxed);
        post(WM_APP_DRAG, kind, pos);
    } else {
        post(WM_APP_BIND, b.idx, pos);
    }
    if mods & (M_SUPER | M_ALT) != 0 {
        mask();
    }
    !b.flags.non_consuming
}

/// Starts the input thread. Hooks stay installed until the process exits.
pub fn start(main: HWND, follow_mouse: bool) {
    let sh = shared();
    sh.main.store(main.0 as isize, Ordering::Relaxed);
    sh.follow_mouse.store(follow_mouse, Ordering::Relaxed);
    std::thread::Builder::new()
        .name("hyprwin-input".into())
        .spawn(|| unsafe {
            let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL);
            shared().thread_id.store(GetCurrentThreadId(), Ordering::Relaxed);
            let hmod = GetModuleHandleW(None).ok().map(|m| m.into());
            let kb = SetWindowsHookExW(WH_KEYBOARD_LL, Some(kb_proc), hmod, 0);
            let ms = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), hmod, 0);
            crate::info!("input hooks installed: keyboard={} mouse={}", kb.is_ok(), ms.is_ok());
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        })
        .expect("spawn input thread");
}

