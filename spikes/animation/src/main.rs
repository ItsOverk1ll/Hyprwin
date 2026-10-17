//! Animation spike — answers "can we make Windows animate like Hyprland?"
//!
//!   animation-spike selftest                 automated: spawns test windows, measures, pixel-checks
//!   animation-spike swap [opts]              swaps your two top-most windows, animated
//!   animation-spike workspace [opts]         slides your visible windows out and back in
//!   opts: --strategy ghost|overlay  --ms N  --naive  --curve x0,y0,x1,y1
//!
//! Real windows are moved exactly once; only DWM thumbnails are animated.
//! Every window we cloak is recorded and uncloaked on exit, panic or Ctrl+C.

use std::process::{Child, Command};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use hyprwin_anim::{Bezier, Rect, WorkspaceStyle, workspace_frame};
use hyprwin_win32::ghost::{Ghost, LayerMode, Overlay, apply_ghost_frames};
use hyprwin_win32::{HWND, clock::FrameClock, cloak, dpi, dwm, monitor, pump_messages, window};
use windows::Win32::Foundation::{COLORREF, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{CreateSolidBrush, GetDC, GetPixel, ReleaseDC};
use windows::Win32::System::Console::SetConsoleCtrlHandler;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, PCWSTR};

// ---------------------------------------------------------------- safety net

static CLOAKED: Mutex<Vec<isize>> = Mutex::new(Vec::new());

fn cloak(h: HWND) -> bool {
    let ok = cloak::set_cloaked(h, true).is_ok();
    if ok {
        CLOAKED.lock().unwrap().push(h.0 as isize);
    }
    ok
}

fn uncloak(h: HWND) {
    let _ = cloak::set_cloaked(h, false);
    CLOAKED.lock().unwrap().retain(|&x| x != h.0 as isize);
}

fn uncloak_all() {
    hyprwin_win32::com_init();
    let list: Vec<isize> = std::mem::take(&mut *CLOAKED.lock().unwrap_or_else(|e| e.into_inner()));
    for h in list {
        let _ = cloak::set_cloaked(HWND(h as *mut _), false);
    }
}

unsafe extern "system" fn on_ctrl(_: u32) -> BOOL {
    uncloak_all();
    false.into()
}

fn install_safety_net() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        uncloak_all();
        prev(info);
    }));
    unsafe {
        let _ = SetConsoleCtrlHandler(Some(on_ctrl), true);
    }
}

// ---------------------------------------------------------------- options

struct Opts {
    ghost: bool,
    ms: f64,
    naive: bool,
    curve: Bezier,
}

fn parse_opts(args: &[String]) -> Opts {
    let mut o = Opts { ghost: true, ms: 400.0, naive: false, curve: Bezier::HYPR_DEFAULT };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--strategy" => o.ghost = it.next().map(|s| s != "overlay").unwrap_or(true),
            "--ms" => o.ms = it.next().and_then(|s| s.parse().ok()).unwrap_or(400.0),
            "--naive" => o.naive = true,
            "--curve" => {
                if let Some(v) = it.next() {
                    let p: Vec<f64> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                    if p.len() == 4 {
                        o.curve = Bezier::new(p[0], p[1], p[2], p[3]);
                    }
                }
            }
            _ => {}
        }
    }
    o
}

// ---------------------------------------------------------------- animation core

/// One window to animate from `from` to `to` (visible-frame coordinates).
struct Job {
    hwnd: HWND,
    from: Rect,
    to: Rect,
    from_opacity: f64,
    to_opacity: f64,
    /// Cloak the real window at the end (it is leaving the screen).
    cloak_after: bool,
}

#[derive(Default, Debug)]
struct Stats {
    frames: u32,
    total_ms: f64,
    worst_work_ms: f64,
    mean_work_ms: f64,
    mean_interval_ms: f64,
    worst_interval_ms: f64,
    late_frames: u32,
    setup_ms: f64,
    teardown_ms: f64,
}

fn source_crop(h: HWND) -> Option<Rect> {
    let outer = window::window_rect(h).ok()?;
    let vis = dwm::frame_bounds(h).ok()?;
    Some(Rect::new(vis.x - outer.x, vis.y - outer.y, vis.w, vis.h))
}

/// The ghost technique:
/// 1. snapshot each window as a live thumbnail at its start rect;
/// 2. cloak the real window and move it to its final rect (once);
/// 3. animate only the thumbnails, paced by the compositor clock;
/// 4. uncloak real windows under the ghosts, wait one frame, remove ghosts.
fn animate(jobs: &[Job], opts: &Opts, monitor_rect: Rect, expected_frame_ms: f64) -> Stats {
    let mut stats = Stats::default();
    let setup = Instant::now();
    let mut clock = FrameClock::new();

    let crops: Vec<Option<Rect>> = jobs.iter().map(|j| source_crop(j.hwnd)).collect();
    let mut ghosts: Vec<Ghost> = Vec::new();
    let mut overlay: Option<Overlay> = None;
    if opts.ghost {
        for (j, crop) in jobs.iter().zip(&crops) {
            ghosts.push(Ghost::new(j.hwnd, j.from, *crop, true).expect("ghost"));
        }
    } else {
        let mut ov = Overlay::new(monitor_rect, LayerMode::ColorKey).expect("overlay");
        for (j, crop) in jobs.iter().zip(&crops) {
            let i = ov.add(j.hwnd, *crop).expect("thumb");
            ov.set(i, j.from, (j.from_opacity * 255.0) as u8).unwrap();
        }
        overlay = Some(ov);
    }
    pump_messages();
    clock.wait(); // ghosts are on screen before the real windows vanish

    for j in jobs {
        cloak(j.hwnd);
        if j.from != j.to {
            let _ = window::set_visible_rect(j.hwnd, j.to);
        }
    }
    stats.setup_ms = setup.elapsed().as_secs_f64() * 1e3;

    let start = Instant::now();
    let mut work_sum = 0.0;
    let mut interval_sum = 0.0;
    clock.wait();
    loop {
        let t = (start.elapsed().as_secs_f64() * 1e3 / opts.ms).min(1.0);
        let e = opts.curve.ease(t);
        let work = Instant::now();
        if let Some(ov) = &overlay {
            for (i, j) in jobs.iter().enumerate() {
                let op = hyprwin_anim::lerp(j.from_opacity, j.to_opacity, e).clamp(0.0, 1.0);
                let _ = ov.set(i, Rect::lerp(j.from, j.to, e), (op * 255.0) as u8);
            }
        } else {
            let frames: Vec<(&Ghost, Rect, u8)> = ghosts
                .iter()
                .zip(jobs)
                .map(|(g, j)| {
                    let op = hyprwin_anim::lerp(j.from_opacity, j.to_opacity, e).clamp(0.0, 1.0);
                    (g, Rect::lerp(j.from, j.to, e), (op * 255.0) as u8)
                })
                .collect();
            let _ = apply_ghost_frames(&frames);
        }
        pump_messages();
        let w = work.elapsed().as_secs_f64() * 1e3;
        work_sum += w;
        stats.worst_work_ms = stats.worst_work_ms.max(w);
        stats.frames += 1;
        if t >= 1.0 {
            break;
        }
        let dt = clock.wait().as_secs_f64() * 1e3;
        interval_sum += dt;
        stats.worst_interval_ms = stats.worst_interval_ms.max(dt);
        if dt > expected_frame_ms * 1.5 {
            stats.late_frames += 1;
        }
    }
    stats.total_ms = start.elapsed().as_secs_f64() * 1e3;

    let teardown = Instant::now();
    for j in jobs {
        if !j.cloak_after {
            uncloak(j.hwnd);
        }
    }
    clock.wait();
    clock.wait(); // let the real windows' first frames land under the ghosts
    drop(ghosts);
    drop(overlay);
    pump_messages();
    stats.teardown_ms = teardown.elapsed().as_secs_f64() * 1e3;

    stats.mean_work_ms = work_sum / stats.frames.max(1) as f64;
    stats.mean_interval_ms = interval_sum / (stats.frames.saturating_sub(1)).max(1) as f64;
    stats
}

/// Baseline for comparison: move the real window every frame.
fn animate_naive(hwnd: HWND, from: Rect, to: Rect, opts: &Opts, expected_frame_ms: f64) -> Stats {
    let mut stats = Stats::default();
    let mut clock = FrameClock::new();
    let start = Instant::now();
    let (mut work_sum, mut interval_sum) = (0.0, 0.0);
    clock.wait();
    loop {
        let t = (start.elapsed().as_secs_f64() * 1e3 / opts.ms).min(1.0);
        let w0 = Instant::now();
        let _ = window::set_visible_rect(hwnd, Rect::lerp(from, to, opts.curve.ease(t)));
        pump_messages();
        let w = w0.elapsed().as_secs_f64() * 1e3;
        work_sum += w;
        stats.worst_work_ms = stats.worst_work_ms.max(w);
        stats.frames += 1;
        if t >= 1.0 {
            break;
        }
        let dt = clock.wait().as_secs_f64() * 1e3;
        interval_sum += dt;
        stats.worst_interval_ms = stats.worst_interval_ms.max(dt);
        if dt > expected_frame_ms * 1.5 {
            stats.late_frames += 1;
        }
    }
    stats.total_ms = start.elapsed().as_secs_f64() * 1e3;
    stats.mean_work_ms = work_sum / stats.frames.max(1) as f64;
    stats.mean_interval_ms = interval_sum / (stats.frames.saturating_sub(1)).max(1) as f64;
    stats
}

// ---------------------------------------------------------------- helpers

fn pixel(x: i32, y: i32) -> (u8, u8, u8) {
    unsafe {
        let dc = GetDC(None);
        let c = GetPixel(dc, x, y).0;
        ReleaseDC(None, dc);
        ((c & 0xFF) as u8, ((c >> 8) & 0xFF) as u8, ((c >> 16) & 0xFF) as u8)
    }
}

fn is_color(p: (u8, u8, u8), want: (u8, u8, u8)) -> bool {
    let d = |a: u8, b: u8| (a as i32 - b as i32).abs() < 40;
    d(p.0, want.0) && d(p.1, want.1) && d(p.2, want.2)
}

fn settle(frames: u32) {
    let mut c = FrameClock::new();
    for _ in 0..frames {
        pump_messages();
        c.wait();
    }
}

fn primary() -> monitor::Monitor {
    monitor::monitors().into_iter().find(|m| m.primary).expect("primary monitor")
}

fn app_windows_on(mon: Rect) -> Vec<HWND> {
    let me = std::process::id();
    window::top_level_windows()
        .into_iter()
        .filter(|&h| {
            if !window::is_visible(h) || window::is_minimized(h) || dwm::cloaked_state(h) != 0 {
                return false;
            }
            let ex = window::ex_style(h);
            let st = window::style(h);
            if ex & WS_EX_TOOLWINDOW.0 != 0 || ex & WS_EX_NOACTIVATE.0 != 0 || st & WS_CAPTION.0 != WS_CAPTION.0 {
                return false;
            }
            if window::title(h).is_empty() || window::pid(h) == me {
                return false;
            }
            let Ok(r) = dwm::frame_bounds(h) else { return false };
            let (cx, cy) = r.center();
            r.w > 200 && r.h > 150 && cx >= mon.x as f64 && cx < mon.right() as f64 && cy >= mon.y as f64 && cy < mon.bottom() as f64
        })
        .collect()
}

fn print_stats(name: &str, s: &Stats, expected: f64) {
    println!(
        "  {name:<28} frames={:>3} total={:>6.1}ms  work mean={:.3}ms worst={:.3}ms  interval mean={:.2}ms (expect {:.2}) worst={:.2}ms late={}  setup={:.1}ms teardown={:.1}ms",
        s.frames, s.total_ms, s.mean_work_ms, s.worst_work_ms, s.mean_interval_ms, expected, s.worst_interval_ms, s.late_frames, s.setup_ms, s.teardown_ms
    );
}

// ---------------------------------------------------------------- target windows (child processes)

/// Optional minimum outer size for test windows (like Steam/Discord enforce).
static TARGET_MIN: std::sync::OnceLock<(i32, i32)> = std::sync::OnceLock::new();

/// Misbehaviour for robustness tests (`HYPRWIN_TARGET_MODE`):
/// `hang` stops processing messages 1.5 s after start (a hung app),
/// `fight` moves itself back whenever it is moved, `titles` renames itself
/// every few milliseconds.
static TARGET_MODE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
static TARGET_HOME: std::sync::OnceLock<(i32, i32, i32, i32)> = std::sync::OnceLock::new();
const TIMER_MISBEHAVE: usize = 77;

unsafe extern "system" fn target_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    let mode = TARGET_MODE.get().map(String::as_str).unwrap_or("");
    if m == WM_TIMER && w.0 == TIMER_MISBEHAVE {
        match mode {
            "hang" => std::thread::sleep(std::time::Duration::from_secs(3600)),
            "titles" => {
                static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let mut t = [0u16; 256];
                let len = unsafe { GetWindowTextW(h, &mut t) } as usize;
                let base: String = String::from_utf16_lossy(&t[..len]).split(" #").next().unwrap_or("").to_string();
                let new = hyprwin_win32::wide(&format!("{base} #{n}"));
                let _ = unsafe { SetWindowTextW(h, PCWSTR(new.as_ptr())) };
            }
            _ => {}
        }
        return LRESULT(0);
    }
    if m == WM_WINDOWPOSCHANGED && mode == "fight" {
        if let Some(&(x, y, ww, hh)) = TARGET_HOME.get() {
            let wp = unsafe { &*(l.0 as *const WINDOWPOS) };
            if (wp.x, wp.y, wp.cx, wp.cy) != (x, y, ww, hh) && wp.flags.0 & SWP_NOMOVE.0 == 0 {
                let _ = unsafe { SetWindowPos(h, None, x, y, ww, hh, SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS) };
            }
        }
    }
    if m == WM_DESTROY {
        unsafe { PostQuitMessage(0) };
        return LRESULT(0);
    }
    if m == WM_GETMINMAXINFO {
        if let Some(&(mw, mh)) = TARGET_MIN.get() {
            let mmi = unsafe { &mut *(l.0 as *mut MINMAXINFO) };
            mmi.ptMinTrackSize.x = mw;
            mmi.ptMinTrackSize.y = mh;
            return LRESULT(0);
        }
    }
    unsafe { DefWindowProcW(h, m, w, l) }
}

/// `target R,G,B X Y W H TITLE [MINW,MINH]`
fn run_target(args: &[String]) {
    dpi::make_per_monitor_aware();
    if let Some(min) = args.get(6) {
        let v: Vec<i32> = min.split(',').filter_map(|x| x.parse().ok()).collect();
        if v.len() == 2 {
            TARGET_MIN.set((v[0], v[1])).ok();
        }
    }
    let rgb: Vec<u32> = args[0].split(',').map(|x| x.parse().unwrap()).collect();
    let n: Vec<i32> = args[1..5].iter().map(|x| x.parse().unwrap()).collect();
    let title = hyprwin_win32::wide(&args[5]);
    let class = hyprwin_win32::wide(&format!("hyprwin.spike.target.{}", args[0]));
    unsafe {
        let wc = WNDCLASSW {
            lpfnWndProc: Some(target_proc),
            hInstance: GetModuleHandleW(None).unwrap().into(),
            lpszClassName: PCWSTR(class.as_ptr()),
            hbrBackground: CreateSolidBrush(COLORREF(rgb[0] | (rgb[1] << 8) | (rgb[2] << 16))),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let h = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            n[0],
            n[1],
            n[2],
            n[3],
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let _ = window::set_visible_rect(h, Rect::new(n[0], n[1], n[2], n[3]));
        if let Ok(mode) = std::env::var("HYPRWIN_TARGET_MODE") {
            if let Ok(r) = window::window_rect(h) {
                TARGET_HOME.set((r.x, r.y, r.w, r.h)).ok();
            }
            let every = match mode.as_str() {
                "hang" => 1500,
                "titles" => 5,
                _ => 0,
            };
            TARGET_MODE.set(mode).ok();
            if every > 0 {
                SetTimer(Some(h), TIMER_MISBEHAVE, every, None);
            }
        }
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

struct Target {
    child: Child,
    hwnd: HWND,
    rgb: (u8, u8, u8),
}

impl Drop for Target {
    fn drop(&mut self) {
        uncloak(self.hwnd);
        let _ = self.child.kill();
    }
}

fn spawn_target(rgb: (u8, u8, u8), r: Rect, title: &str) -> Target {
    let exe = std::env::current_exe().unwrap();
    let child = Command::new(exe)
        .args([
            "target".to_string(),
            format!("{},{},{}", rgb.0, rgb.1, rgb.2),
            r.x.to_string(),
            r.y.to_string(),
            r.w.to_string(),
            r.h.to_string(),
            title.to_string(),
        ])
        .spawn()
        .unwrap();
    let t = hyprwin_win32::wide(title);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(h) = unsafe { FindWindowW(PCWSTR::null(), PCWSTR(t.as_ptr())) } {
            if !h.is_invalid() {
                return Target { child, hwnd: h, rgb };
            }
        }
        assert!(Instant::now() < deadline, "target {title} never appeared");
        std::thread::sleep(Duration::from_millis(20));
    }
}

// ---------------------------------------------------------------- selftest

fn check(name: &str, ok: bool, detail: String, failures: &mut u32) {
    println!("  [{}] {name} {detail}", if ok { "PASS" } else { "FAIL" });
    if !ok {
        *failures += 1;
    }
}

fn selftest(opts: &Opts) -> i32 {
    let mon = primary();
    let m = mon.work;
    let expected = 1000.0 / mon.refresh_hz as f64;
    let clock = FrameClock::new();
    println!("monitor {} {:?} dpi={} {}Hz  clock={}", mon.device, mon.rect, mon.dpi, mon.refresh_hz, clock.source());
    if let Err(e) = cloak::self_test() {
        println!("shell cloak interfaces unavailable: {e}");
        return 2;
    }
    let mut failures = 0;
    let s = mon.scale();
    let sz = |v: f64| (v * s) as i32;
    let r1 = Rect::new(m.x + sz(60.0), m.y + sz(80.0), sz(420.0), sz(300.0));
    let r2 = Rect::new(m.x + sz(560.0), m.y + sz(80.0), sz(420.0), sz(300.0));
    let r3 = Rect::new(m.x + sz(60.0), m.y + sz(440.0), sz(420.0), sz(300.0));
    let spare = Rect::new(m.x + sz(1060.0), m.y + sz(80.0), sz(420.0), sz(300.0));

    let red = spawn_target((230, 20, 20), r1, "hyprwin-spike-red");
    let blue = spawn_target((20, 40, 230), r2, "hyprwin-spike-blue");
    let green = spawn_target((20, 200, 40), r3, "hyprwin-spike-green");
    settle(30);

    // --- geometry
    println!("\n== geometry");
    for t in [&red, &blue, &green] {
        let fb = dwm::frame_bounds(t.hwnd).unwrap();
        let ins = dwm::invisible_insets(t.hwnd).unwrap();
        check("visible frame lands exactly where requested", fb == r1 || fb == r2 || fb == r3, format!("{fb:?} insets={ins:?}"), &mut failures);
    }

    // --- surfaces: does a thumbnail render, is the surface click-through and transparent?
    println!("\n== surfaces");
    let (cx, cy) = (spare.x + spare.w / 2, spare.y + spare.h / 2);
    let before_out = pixel(spare.right() + sz(40.0), cy);
    {
        let g = Ghost::new(red.hwnd, spare, source_crop(red.hwnd), true).unwrap();
        settle(4);
        check("ghost(layered) renders thumbnail", is_color(pixel(cx, cy), red.rgb), format!("{:?}", pixel(cx, cy)), &mut failures);
        check("ghost edge pixel is content (crop correct)", is_color(pixel(spare.x + 2, cy), red.rgb), format!("{:?}", pixel(spare.x + 2, cy)), &mut failures);
        let hit = unsafe { WindowFromPoint(POINT { x: cx, y: cy }) };
        check("ghost is click-through", hit != g.hwnd, format!("hit={hit:?}"), &mut failures);
    }
    {
        let mut ov = Overlay::new(mon.rect, LayerMode::ColorKey).unwrap();
        let i = ov.add(red.hwnd, source_crop(red.hwnd)).unwrap();
        ov.set(i, spare, 255).unwrap();
        settle(4);
        check("overlay renders thumbnail", is_color(pixel(cx, cy), red.rgb), format!("{:?}", pixel(cx, cy)), &mut failures);
        let out = pixel(spare.right() + sz(40.0), cy);
        check("overlay is transparent outside thumbnails", out == before_out, format!("before={before_out:?} now={out:?}"), &mut failures);
        let hit = unsafe { WindowFromPoint(POINT { x: cx, y: cy }) };
        check("overlay is click-through", hit != ov.hwnd, format!("hit={hit:?}"), &mut failures);
    }
    {
        cloak(blue.hwnd);
        settle(3);
        let (bx, by) = (r2.x + r2.w / 2, r2.y + r2.h / 2);
        check("cloaked window is invisible", !is_color(pixel(bx, by), blue.rgb), format!("{:?}", pixel(bx, by)), &mut failures);
        let _g = Ghost::new(blue.hwnd, spare, source_crop(blue.hwnd), true).unwrap();
        settle(4);
        check("thumbnail of cloaked window renders", is_color(pixel(cx, cy), blue.rgb), format!("{:?}", pixel(cx, cy)), &mut failures);
        drop(_g);
        uncloak(blue.hwnd);
        settle(3);
        check("uncloaked window is visible again", is_color(pixel(bx, by), blue.rgb), format!("{:?}", pixel(bx, by)), &mut failures);
    }

    // --- move/swap animation, both strategies
    println!("\n== swap animation ({}ms, curve {:?})", opts.ms, opts.curve);
    for ghost in [true, false] {
        let o = Opts { ghost, ms: opts.ms, naive: false, curve: opts.curve };
        let a = dwm::frame_bounds(red.hwnd).unwrap();
        let b = dwm::frame_bounds(blue.hwnd).unwrap();
        let jobs = [
            Job { hwnd: red.hwnd, from: a, to: b, from_opacity: 1.0, to_opacity: 1.0, cloak_after: false },
            Job { hwnd: blue.hwnd, from: b, to: a, from_opacity: 1.0, to_opacity: 1.0, cloak_after: false },
        ];
        let st = animate(&jobs, &o, mon.rect, expected);
        print_stats(if ghost { "ghost strategy" } else { "overlay strategy" }, &st, expected);
        settle(6);
        let ra = dwm::frame_bounds(red.hwnd).unwrap();
        check("real window ends at target", ra == b, format!("{ra:?}"), &mut failures);
        check("real window uncloaked", dwm::cloaked_state(red.hwnd) == 0, String::new(), &mut failures);
        let (bx, by) = (b.x + b.w / 2, b.y + b.h / 2);
        check("target shows real content", is_color(pixel(bx, by), red.rgb), format!("{:?}", pixel(bx, by)), &mut failures);
    }

    // --- workspace switch: red+blue slide out, green slides in
    println!("\n== workspace switch (slide)");
    cloak(green.hwnd);
    settle(3);
    let mut jobs = Vec::new();
    for (t, incoming) in [(&red, false), (&blue, false), (&green, true)] {
        let rest = dwm::frame_bounds(t.hwnd).unwrap();
        let f0 = workspace_frame(WorkspaceStyle::Slide, rest, mon.rect, 0.0, incoming, true);
        let f1 = workspace_frame(WorkspaceStyle::Slide, rest, mon.rect, 1.0, incoming, true);
        jobs.push(Job { hwnd: t.hwnd, from: f0.rect, to: f1.rect, from_opacity: 1.0, to_opacity: 1.0, cloak_after: !incoming });
    }
    // Real windows stay at their rest rect; only cloak state changes.
    let rests: Vec<Rect> = [&red, &blue, &green].iter().map(|t| dwm::frame_bounds(t.hwnd).unwrap()).collect();
    let st = animate_workspace(&jobs, &rests, opts, mon.rect, expected);
    print_stats("workspace out+in", &st, expected);
    settle(6);
    check("outgoing windows cloaked", dwm::cloaked_state(red.hwnd) != 0 && dwm::cloaked_state(blue.hwnd) != 0, String::new(), &mut failures);
    check("incoming window visible", dwm::cloaked_state(green.hwnd) == 0, String::new(), &mut failures);
    let g = rests[2];
    check("incoming window at rest rect", dwm::frame_bounds(green.hwnd).unwrap() == g, String::new(), &mut failures);
    check("incoming shows content", is_color(pixel(g.x + g.w / 2, g.y + g.h / 2), green.rgb), String::new(), &mut failures);
    uncloak(red.hwnd);
    uncloak(blue.hwnd);
    settle(3);

    // --- naive baseline
    println!("\n== naive baseline (SetWindowPos every frame)");
    let a = dwm::frame_bounds(red.hwnd).unwrap();
    let st = animate_naive(red.hwnd, a, spare, opts, expected);
    print_stats("naive SetWindowPos", &st, expected);

    drop((red, blue, green));
    uncloak_all();
    println!("\n{} failure(s)", failures);
    failures as i32
}

/// Workspace variant: real windows never move, thumbnails slide.
fn animate_workspace(jobs: &[Job], rests: &[Rect], opts: &Opts, mon: Rect, expected: f64) -> Stats {
    // Move "to" for real windows is their rest rect, so build jobs whose real
    // target is the rest rect but whose thumbnail path is from→to.
    let mut stats = Stats::default();
    let mut clock = FrameClock::new();
    let setup = Instant::now();
    let mut ov = Overlay::new(mon, LayerMode::ColorKey).expect("overlay");
    for j in jobs {
        let i = ov.add(j.hwnd, source_crop(j.hwnd)).unwrap();
        ov.set(i, j.from, 255).unwrap();
    }
    pump_messages();
    clock.wait();
    for j in jobs {
        cloak(j.hwnd);
    }
    let _ = rests;
    stats.setup_ms = setup.elapsed().as_secs_f64() * 1e3;
    let start = Instant::now();
    let (mut work_sum, mut interval_sum) = (0.0, 0.0);
    clock.wait();
    loop {
        let t = (start.elapsed().as_secs_f64() * 1e3 / opts.ms).min(1.0);
        let e = opts.curve.ease(t);
        let w0 = Instant::now();
        for (i, j) in jobs.iter().enumerate() {
            let _ = ov.set(i, Rect::lerp(j.from, j.to, e), 255);
        }
        let w = w0.elapsed().as_secs_f64() * 1e3;
        work_sum += w;
        stats.worst_work_ms = stats.worst_work_ms.max(w);
        stats.frames += 1;
        pump_messages();
        if t >= 1.0 {
            break;
        }
        let dt = clock.wait().as_secs_f64() * 1e3;
        interval_sum += dt;
        stats.worst_interval_ms = stats.worst_interval_ms.max(dt);
        if dt > expected * 1.5 {
            stats.late_frames += 1;
        }
    }
    stats.total_ms = start.elapsed().as_secs_f64() * 1e3;
    let td = Instant::now();
    for j in jobs {
        if !j.cloak_after {
            uncloak(j.hwnd);
        }
    }
    clock.wait();
    clock.wait();
    drop(ov);
    pump_messages();
    stats.teardown_ms = td.elapsed().as_secs_f64() * 1e3;
    stats.mean_work_ms = work_sum / stats.frames.max(1) as f64;
    stats.mean_interval_ms = interval_sum / stats.frames.saturating_sub(1).max(1) as f64;
    stats
}

// ---------------------------------------------------------------- interactive

fn swap(opts: &Opts) {
    let mon = primary();
    let expected = 1000.0 / mon.refresh_hz as f64;
    let wins = app_windows_on(mon.work);
    if wins.len() < 2 {
        println!("need two visible app windows on the primary monitor");
        return;
    }
    let (a, b) = (wins[0], wins[1]);
    println!("swapping '{}' <-> '{}'", window::title(a), window::title(b));
    let (ra, rb) = (dwm::frame_bounds(a).unwrap(), dwm::frame_bounds(b).unwrap());
    if opts.naive {
        let s1 = animate_naive(a, ra, rb, opts, expected);
        let s2 = animate_naive(b, rb, ra, opts, expected);
        print_stats("naive a", &s1, expected);
        print_stats("naive b", &s2, expected);
        return;
    }
    let jobs = [
        Job { hwnd: a, from: ra, to: rb, from_opacity: 1.0, to_opacity: 1.0, cloak_after: false },
        Job { hwnd: b, from: rb, to: ra, from_opacity: 1.0, to_opacity: 1.0, cloak_after: false },
    ];
    let st = animate(&jobs, opts, mon.rect, expected);
    print_stats(if opts.ghost { "ghost" } else { "overlay" }, &st, expected);
}

fn workspace(opts: &Opts) {
    let mon = primary();
    let expected = 1000.0 / mon.refresh_hz as f64;
    let wins = app_windows_on(mon.work);
    println!("sliding {} window(s) out and back", wins.len());
    let rests: Vec<Rect> = wins.iter().map(|&h| dwm::frame_bounds(h).unwrap()).collect();
    let out: Vec<Job> = wins
        .iter()
        .zip(&rests)
        .map(|(&h, &r)| Job {
            hwnd: h,
            from: r,
            to: workspace_frame(WorkspaceStyle::Slide, r, mon.rect, 1.0, false, true).rect,
            from_opacity: 1.0,
            to_opacity: 1.0,
            cloak_after: true,
        })
        .collect();
    print_stats("slide out", &animate_workspace(&out, &rests, opts, mon.rect, expected), expected);
    std::thread::sleep(Duration::from_millis(500));
    let back: Vec<Job> = wins
        .iter()
        .zip(&rests)
        .map(|(&h, &r)| Job {
            hwnd: h,
            from: workspace_frame(WorkspaceStyle::Slide, r, mon.rect, 0.0, true, false).rect,
            to: r,
            from_opacity: 1.0,
            to_opacity: 1.0,
            cloak_after: false,
        })
        .collect();
    print_stats("slide in", &animate_workspace(&back, &rests, opts, mon.rect, expected), expected);
    uncloak_all();
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("target") {
        return run_target(&args[1..]);
    }
    dpi::make_per_monitor_aware();
    hyprwin_win32::com_init();
    install_safety_net();
    let opts = parse_opts(&args);
    let code = match args.first().map(String::as_str) {
        Some("selftest") => selftest(&opts),
        Some("swap") => {
            swap(&opts);
            0
        }
        Some("workspace") => {
            workspace(&opts);
            0
        }
        _ => {
            println!("usage: animation-spike selftest|swap|workspace [--strategy ghost|overlay] [--ms N] [--naive] [--curve x0,y0,x1,y1]");
            0
        }
    };
    uncloak_all();
    cloak::release();
    std::process::exit(code);
}
