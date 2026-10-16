//! hyprwin daemon.
//!
//! Threads: main (message loop, WinEvent hooks, all WM state), input (LL hooks),
//! ipc-requests, ipc-events, config-watch. Idle = every thread blocked in the kernel.

#![windows_subsystem = "windows"]

mod app;
mod classify;
mod input;
mod log;
mod shell;
mod tray;

use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use app::App;
use hyprwin_config::Config;
use hyprwin_win32::ledger::Ledger;
use hyprwin_win32::{HWND, cloak, dpi};
use windows::Win32::Foundation::{GetLastError, LPARAM, LRESULT, WPARAM};
use windows::Win32::Storage::FileSystem::{FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_FILE_NAME, FindFirstChangeNotificationW, FindNextChangeNotification};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{CreateMutexW, INFINITE, WaitForSingleObject};
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::PCWSTR;

const WM_APP_EVENTS: u32 = WM_APP + 10;
const WM_APP_IPC: u32 = WM_APP + 11;
const WM_APP_RELOAD: u32 = WM_APP + 12;
const WM_APP_REPOST: u32 = WM_APP + 13;

type IpcJob = (String, Sender<String>);

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    static QUEUE: RefCell<VecDeque<(u32, isize)>> = const { RefCell::new(VecDeque::new()) };
    static QUEUE_POSTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

static MAIN_HWND: OnceLock<isize> = OnceLock::new();
static IPC_RX: OnceLock<Mutex<Receiver<IpcJob>>> = OnceLock::new();
static TASKBAR_CREATED: OnceLock<u32> = OnceLock::new();

fn main_hwnd() -> HWND {
    HWND(*MAIN_HWND.get().unwrap_or(&0) as *mut _)
}

fn post(msg: u32, w: usize, l: isize) {
    unsafe {
        let _ = PostMessageW(Some(main_hwnd()), msg, WPARAM(w), LPARAM(l));
    }
}

/// Runs `f` on the app; if it is busy (re-entrant message), re-posts later.
fn with_app(msg: u32, w: WPARAM, l: LPARAM, f: impl FnOnce(&mut App)) {
    APP.with(|cell| match cell.try_borrow_mut() {
        Ok(mut g) => {
            if let Some(app) = g.as_mut() {
                f(app);
                if app.quit {
                    unsafe { PostQuitMessage(0) };
                }
            }
        }
        Err(_) => post(WM_APP_REPOST, msg as usize, ((w.0 as isize) << 16) ^ l.0),
    });
}

unsafe extern "system" fn win_event(_: HWINEVENTHOOK, event: u32, hwnd: HWND, id_object: i32, id_child: i32, _: u32, _: u32) {
    if id_object != 0 || id_child != 0 || hwnd.is_invalid() {
        return; // only whole windows (OBJID_WINDOW, CHILDID_SELF)
    }
    QUEUE.with(|q| q.borrow_mut().push_back((event, hwnd.0 as isize)));
    if !QUEUE_POSTED.with(|p| p.replace(true)) {
        post(WM_APP_EVENTS, 0, 0);
    }
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        WM_APP_EVENTS => {
            QUEUE_POSTED.with(|p| p.set(false));
            let mut busy = false;
            APP.with(|cell| busy = cell.try_borrow_mut().is_err());
            if busy {
                post(WM_APP_EVENTS, 0, 0);
                return LRESULT(0);
            }
            loop {
                let Some((ev, hw)) = QUEUE.with(|q| q.borrow_mut().pop_front()) else { break };
                with_app(msg, w, l, |a| a.on_winevent(ev, HWND(hw as *mut _)));
            }
        }
        input::WM_APP_BIND => with_app(msg, w, l, |a| a.on_bind(w.0)),
        input::WM_APP_MOUSE => with_app(msg, w, l, |a| a.on_mouse_move()),
        input::WM_APP_DRAG => {
            let (x, y) = input::unpack(l.0 as u64);
            with_app(msg, w, l, |a| a.on_drag(w.0, x, y));
        }
        WM_APP_IPC => {
            let jobs: Vec<IpcJob> = IPC_RX.get().map(|rx| rx.lock().unwrap().try_iter().collect()).unwrap_or_default();
            for (req, reply) in jobs {
                with_app(msg, w, l, |a| {
                    let _ = reply.send(a.handle_request(&req));
                });
            }
        }
        WM_APP_RELOAD => with_app(msg, w, l, |a| a.reload()),
        tray::WM_APP_TRAY => {
            // NOTIFYICON_VERSION_4: the event is in the low word of lParam.
            let event = (l.0 as u32) & 0xFFFF;
            const NIN_SELECT: u32 = WM_USER;
            const NIN_KEYSELECT: u32 = WM_USER + 1;
            match event {
                WM_CONTEXTMENU | NIN_SELECT | NIN_KEYSELECT => {
                    let (paused, groups) = APP.with(|c| c.try_borrow().ok().and_then(|g| g.as_ref().map(|a| (a.paused, app::list_groups())))).unwrap_or((false, Vec::new()));
                    if let Some(cmd) = tray::show_menu(h, paused, &groups) {
                        with_app(msg, w, l, |a| a.on_tray(cmd));
                    }
                }
                WM_LBUTTONDBLCLK => shell::open_settings("settings", None),
                _ => {}
            }
        }
        WM_APP_REPOST => {
            // A deferred message; re-dispatch the cheap ones that carry no payload.
            match w.0 as u32 {
                input::WM_APP_MOUSE => post(input::WM_APP_MOUSE, 0, 0),
                WM_TIMER => post(WM_TIMER, app::TIMER_VERIFY, 0),
                _ => {}
            }
        }
        WM_TIMER => match w.0 {
            app::TIMER_VERIFY => with_app(msg, w, l, |a| a.verify()),
            app::TIMER_SAVE => with_app(msg, w, l, |a| a.save_state()),
            app::TIMER_GROUPS => {
                unsafe {
                    let _ = KillTimer(Some(h), app::TIMER_GROUPS);
                }
                with_app(msg, w, l, |a| a.check_groups());
            }
            app::TIMER_MONITORS => {
                unsafe {
                    let _ = KillTimer(Some(h), app::TIMER_MONITORS);
                }
                with_app(msg, w, l, |a| a.refresh_monitors());
            }
            _ => {}
        },
        WM_DISPLAYCHANGE | WM_DPICHANGED | WM_DEVICECHANGE => unsafe {
            SetTimer(Some(h), app::TIMER_MONITORS, 500, None);
        },
        WM_SETTINGCHANGE if w.0 as u32 == SPI_SETWORKAREA.0 => unsafe {
            SetTimer(Some(h), app::TIMER_MONITORS, 500, None);
        },
        WM_POWERBROADCAST => unsafe {
            SetTimer(Some(h), app::TIMER_MONITORS, 1500, None);
        },
        WM_QUERYENDSESSION => return LRESULT(1),
        WM_ENDSESSION => {
            if w.0 != 0 {
                shutdown();
            }
        }
        m if Some(&m) == TASKBAR_CREATED.get() => {
            info!("explorer restarted; refreshing shell interfaces and tray icon");
            cloak::release();
            let paused = APP.with(|c| c.try_borrow().ok().and_then(|g| g.as_ref().map(|a| a.paused))).unwrap_or(false);
            tray::add(h, paused);
        }
        _ => return unsafe { DefWindowProcW(h, msg, w, l) },
    }
    LRESULT(0)
}

fn create_main_window() -> HWND {
    unsafe {
        let class = hyprwin_win32::wide("hyprwin.main");
        let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: GetModuleHandleW(None).unwrap().into(), lpszClassName: PCWSTR(class.as_ptr()), ..Default::default() };
        RegisterClassW(&wc);
        // A real (hidden) top-level window, so we receive display broadcasts.
        CreateWindowExW(WS_EX_TOOLWINDOW, PCWSTR(class.as_ptr()), PCWSTR(class.as_ptr()), WS_POPUP, 0, 0, 0, 0, None, None, None, None).expect("main window")
    }
}

fn install_win_event_hooks() {
    let ranges = [
        (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
        (EVENT_SYSTEM_MOVESIZESTART, EVENT_SYSTEM_MINIMIZEEND),
        (EVENT_OBJECT_CREATE, EVENT_OBJECT_HIDE),
        (EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_NAMECHANGE),
        (EVENT_OBJECT_CLOAKED, EVENT_OBJECT_UNCLOAKED),
    ];
    for (a, b) in ranges {
        let h = unsafe { SetWinEventHook(a, b, None, Some(win_event), 0, 0, WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS) };
        if h.is_invalid() {
            error!("SetWinEventHook {a:#x}..{b:#x} failed");
        }
    }
}

// ---------------------------------------------------------------- config

pub fn config_candidates() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(home) = std::env::var_os("USERPROFILE") {
        v.push(PathBuf::from(home).join(".config").join("hypr").join("hyprland.conf"));
    }
    if let Some(appdata) = std::env::var_os("APPDATA") {
        v.push(PathBuf::from(appdata).join("hyprwin").join("hyprland.conf"));
    }
    v
}

fn find_or_create_config(explicit: Option<PathBuf>) -> PathBuf {
    if let Some(p) = explicit {
        return p;
    }
    let cands = config_candidates();
    if let Some(p) = cands.iter().find(|p| p.exists()) {
        return p.clone();
    }
    // Like Hyprland: write the default config on first run.
    let target = cands.last().cloned().unwrap_or_else(|| PathBuf::from("hyprland.conf"));
    if let Some(dir) = target.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match std::fs::write(&target, hyprwin_config::DEFAULT_CONFIG) {
        Ok(()) => info!("wrote default config to {}", target.display()),
        Err(e) => warn!("cannot write default config: {e}"),
    }
    target
}

pub fn load_config(path: &Path) -> Config {
    if path.exists() { Config::load_file(path) } else { Config::load_str(hyprwin_config::DEFAULT_CONFIG) }
}

fn watch_config(files: Vec<PathBuf>) {
    let dirs: Vec<PathBuf> = {
        let mut d: Vec<PathBuf> = files.iter().filter_map(|f| f.parent().map(Path::to_path_buf)).collect();
        d.sort();
        d.dedup();
        d
    };
    for dir in dirs {
        std::thread::Builder::new()
            .name("hyprwin-config-watch".into())
            .spawn(move || unsafe {
                let w = hyprwin_win32::wide(&dir.to_string_lossy());
                let Ok(h) = FindFirstChangeNotificationW(PCWSTR(w.as_ptr()), false, FILE_NOTIFY_CHANGE_LAST_WRITE | FILE_NOTIFY_CHANGE_FILE_NAME) else { return };
                loop {
                    WaitForSingleObject(h, INFINITE);
                    // Editors write in bursts; coalesce.
                    std::thread::sleep(Duration::from_millis(150));
                    post(WM_APP_RELOAD, 0, 0);
                    if FindNextChangeNotification(h).is_err() {
                        break;
                    }
                }
            })
            .ok();
    }
}

// ---------------------------------------------------------------- IPC threads

fn start_ipc(tx: Sender<IpcJob>, events: Receiver<String>) {
    let Some(sec) = hyprwin_ipc::PipeSecurity::current_user() else {
        error!("cannot build pipe security descriptor; IPC disabled");
        return;
    };
    let sec = Arc::new(sec);
    let s1 = sec.clone();
    std::thread::Builder::new()
        .name("hyprwin-ipc".into())
        .spawn(move || {
            let name = hyprwin_ipc::request_pipe_name();
            loop {
                let Ok(mut f) = hyprwin_ipc::accept(&name, &s1, false) else {
                    std::thread::sleep(Duration::from_millis(200));
                    continue;
                };
                let Ok(req) = hyprwin_ipc::read_request(&mut f) else { continue };
                let (rtx, rrx) = channel();
                if tx.send((req, rtx)).is_err() {
                    return;
                }
                post(WM_APP_IPC, 0, 0);
                let reply = rrx.recv_timeout(Duration::from_secs(3)).unwrap_or_else(|_| "error: daemon busy".into());
                let _ = f.write_all(reply.as_bytes());
                let _ = f.flush();
            }
        })
        .ok();

    let subscribers: Arc<Mutex<Vec<std::fs::File>>> = Arc::new(Mutex::new(Vec::new()));
    let subs = subscribers.clone();
    let s2 = sec.clone();
    std::thread::Builder::new()
        .name("hyprwin-ipc-events-accept".into())
        .spawn(move || {
            let name = hyprwin_ipc::event_pipe_name();
            loop {
                match hyprwin_ipc::accept(&name, &s2, true) {
                    Ok(f) => subs.lock().unwrap().push(f),
                    Err(_) => std::thread::sleep(Duration::from_millis(200)),
                }
            }
        })
        .ok();
    std::thread::Builder::new()
        .name("hyprwin-ipc-events-write".into())
        .spawn(move || {
            for line in events {
                let mut subs = subscribers.lock().unwrap();
                subs.retain_mut(|f| f.write_all(line.as_bytes()).is_ok());
            }
        })
        .ok();
}

// ---------------------------------------------------------------- lifecycle

fn shutdown() {
    APP.with(|cell| {
        if let Ok(mut g) = cell.try_borrow_mut() {
            if let Some(app) = g.as_mut() {
                for c in app.cfg.exec_shutdown.clone() {
                    shell::exec(&c, &app.cfg.env);
                }
                app.save_state();
                app.restore_everything();
            }
        } else if let Some(mut l) = Ledger::load() {
            l.restore_all();
            let _ = l.save();
        }
    });
    cloak::release();
}

fn spawn_guard() {
    let Ok(exe) = std::env::current_exe() else { return };
    let guard = exe.with_file_name("hyprwin-guard.exe");
    if !guard.exists() {
        warn!("hyprwin-guard.exe not found next to the daemon; crash recovery relies on the panic hook only");
        return;
    }
    match std::process::Command::new(&guard).arg(std::process::id().to_string()).spawn() {
        Ok(_) => info!("guard started"),
        Err(e) => warn!("cannot start guard: {e}"),
    }
}

fn main() {
    let log_path = log::init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version") {
        return;
    }
    let explicit_cfg = args.iter().position(|a| a == "--config" || a == "-c").and_then(|i| args.get(i + 1)).map(PathBuf::from);

    // Single instance per user session.
    let name = hyprwin_win32::wide("Local\\hyprwin-daemon");
    let _mutex = unsafe { CreateMutexW(None, true, PCWSTR(name.as_ptr())) };
    if unsafe { GetLastError() }.0 == 183 {
        // ERROR_ALREADY_EXISTS
        log::write(log::WARN, format_args!("hyprwin is already running"));
        return;
    }

    dpi::make_per_monitor_aware();
    hyprwin_win32::com_init();
    info!("hyprwin {} starting (pid {}, log {})", env!("CARGO_PKG_VERSION"), std::process::id(), log_path.display());

    // A previous run that died without cleaning up: put everything back first.
    if let Some(mut stale) = Ledger::load() {
        if !stale.is_empty() {
            let n = stale.restore_all();
            warn!("restored {n} window(s) left hidden by a previous run (pid {})", stale.daemon_pid);
        }
    }

    // Panic: restore everything before the process dies.
    std::panic::set_hook(Box::new(|info| {
        log::write(log::ERROR, format_args!("PANIC: {info}"));
        if let Some(mut l) = Ledger::load() {
            l.restore_all();
            let _ = l.save();
        }
    }));

    match cloak::self_test() {
        Ok(()) => info!("shell cloak interfaces OK"),
        Err(e) => warn!("shell cloak unavailable ({e}); falling back to off-screen parking"),
    }

    let cfg_path = find_or_create_config(explicit_cfg);
    let cfg = load_config(&cfg_path);
    for w in &cfg.warnings {
        warn!("config: {w}");
    }
    info!("config {} ({} binds, {} rules)", cfg_path.display(), cfg.binds.len(), cfg.window_rules.len());

    let main = create_main_window();
    MAIN_HWND.set(main.0 as isize).ok();
    let tb = hyprwin_win32::wide("TaskbarCreated");
    TASKBAR_CREATED.set(unsafe { RegisterWindowMessageW(PCWSTR(tb.as_ptr())) }).ok();

    let (ipc_tx, ipc_rx) = channel();
    IPC_RX.set(Mutex::new(ipc_rx)).ok();
    let (ev_tx, ev_rx) = channel();
    start_ipc(ipc_tx, ev_rx);
    watch_config(cfg.files.clone());
    input::start(main, cfg.follow_mouse != 0);

    let exec_once = cfg.exec_once.clone();
    let exec_always = cfg.exec.clone();
    let env = cfg.env.clone();
    let mut app = App::new(cfg, cfg_path, main, ev_tx);
    install_win_event_hooks();
    app.manage_existing();
    APP.with(|c| *c.borrow_mut() = Some(app));
    for c in exec_once.iter().chain(&exec_always) {
        shell::exec(c, &env);
    }
    spawn_guard();
    hyprwin_config::themes::install_builtins();
    tray::add(main, false);
    info!("ready");

    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    info!("exiting");
    tray::remove(main);
    shutdown();
    std::process::exit(0);
}
