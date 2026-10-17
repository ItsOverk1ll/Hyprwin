//! Randomized stress test of the window-manager model: long random sequences
//! of user and OS operations, with the model's invariants checked after every
//! step. Deterministic (fixed seeds), so a failure is reproducible: the panic
//! message names the seed and step, and `STRESS_SEED=<seed>` reruns just it.

use hyprwin_layout::master::{NewStatus, Orientation};
use hyprwin_layout::wm::{MonitorSpec, NewWindow};
use hyprwin_layout::{Dir, Effect, FullscreenMode, Gaps, LayoutKind, Rect, WinId, Wm, WmConfig, WsSel};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
    fn range(&mut self, lo: i32, hi: i32) -> i32 {
        lo + self.below((hi - lo + 1) as u64) as i32
    }
    fn pick<T: Copy>(&mut self, v: &[T]) -> Option<T> {
        if v.is_empty() { None } else { Some(v[self.below(v.len() as u64) as usize]) }
    }
}

fn mon(name: &str, x: i32, w: i32, h: i32) -> MonitorSpec {
    MonitorSpec { name: name.into(), rect: Rect::new(x, 0, w, h), work: Rect::new(x, 0, w, h - 48), scale: 1.0 }
}

fn monitor_set(rng: &mut Rng) -> Vec<MonitorSpec> {
    match rng.below(4) {
        0 => vec![mon("A", 0, 1920, 1080)],
        1 => vec![mon("A", 0, 1920, 1080), mon("B", 1920, 2560, 1440)],
        2 => vec![mon("B", 1920, 2560, 1440)],
        _ => vec![mon("A", 0, 1920, 1080), mon("B", 1920, 2560, 1440), mon("C", -1280, 1280, 1024)],
    }
}

fn overlap(a: Rect, b: Rect) -> bool {
    a.x < b.right() && b.x < a.right() && a.y < b.bottom() && b.y < a.bottom()
}

fn is_visible_ws(wm: &Wm, ws: i64) -> bool {
    wm.monitors.iter().any(|m| m.active == ws || m.special == Some(ws))
}

/// Everything that must hold after any operation.
fn check(wm: &mut Wm, ctx: &str) {
    assert!(!wm.monitors.is_empty(), "{ctx}: no monitors");
    assert!(wm.focused_monitor < wm.monitors.len(), "{ctx}: focused monitor out of range");
    for (i, m) in wm.monitors.iter().enumerate() {
        let ws = wm.workspaces.get(&m.active).unwrap_or_else(|| panic!("{ctx}: monitor {i} shows missing workspace {}", m.active));
        assert!(m.active > 0, "{ctx}: monitor {i} active workspace is special");
        assert_eq!(ws.monitor, i, "{ctx}: active workspace {} of monitor {i} says monitor {}", m.active, ws.monitor);
        if let Some(s) = m.special {
            assert!(s < 0, "{ctx}: monitor {i} special {s} is not special");
            let sw = wm.workspaces.get(&s).unwrap_or_else(|| panic!("{ctx}: monitor {i} shows missing special {s}"));
            assert_eq!(sw.monitor, i, "{ctx}: special {s} on monitor {i} says monitor {}", sw.monitor);
        }
    }
    for (a, ma) in wm.monitors.iter().enumerate() {
        for mb in &wm.monitors[a + 1..] {
            assert_ne!(ma.active, mb.active, "{ctx}: workspace {} active on two monitors", ma.active);
        }
    }
    for ws in wm.workspaces.values() {
        assert!(ws.monitor < wm.monitors.len(), "{ctx}: workspace {} on missing monitor {}", ws.id, ws.monitor);
        for id in ws.layout_windows() {
            let w = wm.windows.get(&id).unwrap_or_else(|| panic!("{ctx}: workspace {} layout holds missing window {id}", ws.id));
            assert_eq!(w.ws, ws.id, "{ctx}: window {id} is in the layout of {} but lives on {}", ws.id, w.ws);
            assert!(!w.floating, "{ctx}: floating window {id} is in the layout of {}", ws.id);
        }
        let mut seen = ws.layout_windows();
        let n = seen.len();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), n, "{ctx}: workspace {} layout lists a window twice", ws.id);
        if let Some((f, _)) = ws.fullscreen {
            let w = wm.windows.get(&f).unwrap_or_else(|| panic!("{ctx}: workspace {} fullscreen window {f} is gone", ws.id));
            assert_eq!(w.ws, ws.id, "{ctx}: workspace {} fullscreen window {f} lives on {}", ws.id, w.ws);
        }
    }
    for w in wm.windows.values() {
        let ws = wm.workspaces.get(&w.ws).unwrap_or_else(|| panic!("{ctx}: window {} on missing workspace {}", w.id, w.ws));
        // (Minimized windows may lose their slot; restoring re-inserts them.)
        if !w.floating && !w.minimized {
            assert!(ws.layout_windows().contains(&w.id), "{ctx}: tiled window {} missing from the layout of {}", w.id, w.ws);
        }
        if w.pinned {
            assert!(w.floating, "{ctx}: pinned window {} is tiled", w.id);
        }
    }
    if let Some(f) = wm.focus {
        let w = wm.windows.get(&f).unwrap_or_else(|| panic!("{ctx}: focus on missing window {f}"));
        assert!(!w.minimized, "{ctx}: focus on minimized window {f}");
    }
    // Visible tiled windows on one workspace never overlap and stay on their monitor.
    for ws in wm.workspaces.values() {
        if !is_visible_ws(wm, ws.id) || ws.fullscreen.is_some() {
            continue;
        }
        let mon = wm.monitors[ws.monitor].spec.rect;
        let tiled: Vec<(WinId, Rect)> = ws.layout_windows().into_iter().filter_map(|id| wm.desired_rect(id).map(|r| (id, r))).collect();
        // Minimum sizes that cannot all fit are the daemon's job (overflow).
        let fits = tiled.iter().all(|(id, r)| {
            let m = wm.windows[id].min_size;
            r.w + 2 >= m.0 && r.h + 2 >= m.1
        });
        if !fits {
            continue;
        }
        for (i, (a, ra)) in tiled.iter().enumerate() {
            assert!(ra.w > 0 && ra.h > 0, "{ctx}: window {a} has empty tile {ra:?}");
            assert!(ra.x >= mon.x && ra.y >= mon.y && ra.right() <= mon.right() && ra.bottom() <= mon.bottom(), "{ctx}: window {a} tile {ra:?} leaves monitor {mon:?}");
            for (b, rb) in &tiled[i + 1..] {
                assert!(
                    !overlap(*ra, *rb),
                    "{ctx}: tiles of {a} {ra:?} and {b} {rb:?} overlap on workspace {}\nlayout: {}\nmins: {:?}",
                    ws.id,
                    wm.workspace_layout_json(ws.id),
                    tiled.iter().map(|(id, _)| (id, wm.windows[id].min_size, wm.windows[id].minimized)).collect::<Vec<_>>()
                );
            }
        }
    }
    // Hidden = not minimized and not on a shown workspace.
    let mut hidden = wm.hidden_windows();
    hidden.sort();
    let mut expect: Vec<WinId> = wm.windows.values().filter(|w| !w.minimized && !is_visible_ws(wm, w.ws)).map(|w| w.id).collect();
    expect.sort();
    assert_eq!(hidden, expect, "{ctx}: hidden windows disagree with the visible workspaces");
    // sync is idempotent: nothing left to do right after an operation.
    let again = wm.sync();
    let busy: Vec<&Effect> = again.iter().filter(|e| matches!(e, Effect::Place { .. } | Effect::Show(_) | Effect::Hide(_) | Effect::Focus(_))).collect();
    assert!(busy.is_empty(), "{ctx}: second sync still emits {busy:?}");
}

fn new_window(rng: &mut Rng, wm: &Wm) -> NewWindow {
    let x = rng.range(-1000, 4000);
    NewWindow {
        rect: Rect::new(x, rng.range(0, 900), rng.range(200, 1600), rng.range(150, 1000)),
        floating: rng.chance(20),
        monitor: if rng.chance(30) { Some(rng.below(wm.monitors.len() as u64) as usize) } else { None },
        workspace: if rng.chance(10) { WsSel::parse(&rng.range(1, 6).to_string()) } else { None },
        fullscreen: rng.chance(3),
        pseudo: rng.chance(5),
        no_initial_focus: rng.chance(10),
        cursor: if rng.chance(50) { Some((rng.range(-1280, 4480) as f64, rng.range(0, 1440) as f64)) } else { None },
        float_rect: None,
        center: rng.chance(10),
        pin: rng.chance(5),
    }
}

fn sel(rng: &mut Rng) -> WsSel {
    match rng.below(10) {
        0 => WsSel::Relative(if rng.chance(50) { 1 } else { -1 }),
        1 => WsSel::Previous,
        2 => WsSel::Empty,
        3 => WsSel::MonitorRelative(1),
        4 => WsSel::OpenRelative(-1),
        5 => WsSel::Name("web".into()),
        _ => WsSel::Id(rng.range(1, 6) as i64),
    }
}

fn dir(rng: &mut Rng) -> Dir {
    [Dir::Left, Dir::Right, Dir::Up, Dir::Down][rng.below(4) as usize]
}

fn step(wm: &mut Wm, rng: &mut Rng, next_id: &mut WinId) -> String {
    let ids: Vec<WinId> = wm.windows.keys().copied().collect();
    let any = rng.pick(&ids);
    match rng.below(34) {
        0..=4 => {
            *next_id += 1;
            let nw = new_window(rng, wm);
            let s = format!("add_window({next_id}, {nw:?})");
            wm.add_window(*next_id, nw);
            s
        }
        5 | 6 => match any {
            Some(id) => {
                wm.remove_window(id);
                format!("remove_window({id})")
            }
            None => "noop".into(),
        },
        7 => match any {
            Some(id) => {
                let m = rng.chance(60);
                wm.set_minimized(id, m);
                format!("set_minimized({id}, {m})")
            }
            None => "noop".into(),
        },
        8 => match any {
            Some(id) => {
                wm.on_os_focus(id);
                format!("on_os_focus({id})")
            }
            None => "noop".into(),
        },
        9 | 10 => {
            let s = sel(rng);
            let t = format!("switch_workspace({s:?})");
            wm.switch_workspace(&s);
            t
        }
        11 => {
            let s = WsSel::Special(if rng.chance(50) { None } else { Some("pad".into()) });
            let t = format!("toggle_special({s:?})");
            wm.toggle_special(&s);
            t
        }
        12 | 13 => {
            let s = if rng.chance(15) { WsSel::Special(None) } else { sel(rng) };
            let follow = rng.chance(50);
            let w = if rng.chance(50) { any } else { None };
            let t = format!("move_to_workspace({w:?}, {s:?}, {follow})");
            wm.move_to_workspace(w, &s, follow);
            t
        }
        14 => {
            let d = dir(rng);
            wm.focus_dir(d);
            format!("focus_dir({d:?})")
        }
        15 => {
            let d = dir(rng);
            wm.move_dir(d);
            format!("move_dir({d:?})")
        }
        16 => {
            let d = dir(rng);
            wm.swap_dir(d);
            format!("swap_dir({d:?})")
        }
        17 => {
            let f = [None, Some(true), Some(false)][rng.below(3) as usize];
            let w = if rng.chance(50) { any } else { None };
            wm.set_floating(w, f);
            format!("set_floating({w:?}, {f:?})")
        }
        18 => {
            let mode = if rng.chance(50) { FullscreenMode::Fullscreen } else { FullscreenMode::Maximized };
            let a = [None, Some(true), Some(false)][rng.below(3) as usize];
            wm.fullscreen(mode, a);
            format!("fullscreen({mode:?}, {a:?})")
        }
        19 => {
            wm.pin(None);
            "pin(None)".into()
        }
        20 => {
            wm.pseudo(None);
            "pseudo(None)".into()
        }
        21 => {
            let (dx, dy) = (rng.range(-300, 300), rng.range(-300, 300));
            wm.resize_active(dx, dy, false);
            format!("resize_active({dx}, {dy})")
        }
        22 => {
            let msg = ["togglesplit", "swapsplit", "preselect l", "splitratio 0.3", "splitratio -0.2", "addmaster", "removemaster", "orientationtop", "orientationright", "swapwithmaster", "focusmaster", "cyclenext", "swapprev", "mfact 0.7"][rng.below(14) as usize];
            wm.layout_msg(msg);
            format!("layout_msg({msg:?})")
        }
        23 => {
            let ws = wm.active_workspace();
            let k = if rng.chance(50) { LayoutKind::Master } else { LayoutKind::Dwindle };
            wm.set_layout(ws, k);
            format!("set_layout({ws}, {k:?})")
        }
        24 => match any {
            Some(id) => {
                let c = (rng.range(-1280, 4480) as f64, rng.range(0, 1440) as f64);
                let r = Rect::new(c.0 as i32 - 300, c.1 as i32 - 200, 600, 400);
                wm.on_user_move(id, r, c);
                format!("on_user_move({id}, {r:?}, {c:?})")
            }
            None => "noop".into(),
        },
        25 => match any.and_then(|id| wm.desired_rect(id).map(|r| (id, r))) {
            Some((id, r)) => {
                let n = Rect::new(r.x + rng.range(-100, 100), r.y, (r.w + rng.range(-200, 200)).max(50), (r.h + rng.range(-200, 200)).max(50));
                wm.on_user_resize(id, n);
                format!("on_user_resize({id}, {n:?})")
            }
            None => "noop".into(),
        },
        26 => {
            let ms = monitor_set(rng);
            let t = format!("set_monitors({:?})", ms.iter().map(|m| m.name.as_str()).collect::<Vec<_>>());
            wm.set_monitors(ms);
            t
        }
        27 => {
            let s = ["+1", "-1", "l", "r", "A", "B"][rng.below(6) as usize];
            wm.focus_monitor(s);
            format!("focus_monitor({s})")
        }
        28 => {
            let s = ["+1", "-1", "l", "r", "A", "B"][rng.below(6) as usize];
            let ws = if rng.chance(50) { Some(sel(rng)) } else { None };
            let t = format!("move_workspace_to_monitor({ws:?}, {s})");
            wm.move_workspace_to_monitor(ws.as_ref(), s);
            t
        }
        29 => {
            wm.swap_active_workspaces("A", "B");
            "swap_active_workspaces(A, B)".into()
        }
        30 => match any {
            Some(id) => {
                let (w, h) = (rng.range(300, 1500), rng.range(200, 900));
                wm.set_min_size(id, w, h);
                let ws = wm.windows[&id].ws;
                wm.overflow_ws(ws);
                format!("set_min_size({id}, {w}, {h}) + overflow_ws({ws})")
            }
            None => "noop".into(),
        },
        31 => {
            let f = rng.chance(50);
            wm.cycle(f, false, false);
            wm.swap_next(!f);
            format!("cycle({f}) + swap_next({})", !f)
        }
        32 => {
            wm.focus_current_or_last();
            "focus_current_or_last".into()
        }
        _ => {
            // Daemon restart: a fresh model restored from a snapshot.
            let snap = wm.snapshot();
            let specs: Vec<MonitorSpec> = wm.monitors.iter().map(|m| m.spec.clone()).collect();
            let mut fresh = Wm::new(wm.cfg.clone(), specs);
            for w in wm.windows.values() {
                let rect = wm.desired_rect(w.id).unwrap_or(w.float_rect);
                fresh.add_window(w.id, NewWindow { rect, floating: w.floating, ..Default::default() });
                if w.minimized {
                    fresh.set_minimized(w.id, true);
                }
            }
            let t = std::time::Instant::now();
            fresh.restore(&snap);
            if std::env::var_os("STRESS_TIME").is_some() {
                eprintln!("OP {} {} restore-only", t.elapsed().as_micros(), wm.windows.len());
            }
            *wm = fresh;
            "snapshot + restore into a fresh model".into()
        }
    }
}

fn steps() -> usize {
    std::env::var("STRESS_STEPS").ok().and_then(|s| s.parse().ok()).unwrap_or(400)
}

fn run(seed: u64, steps: usize) {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut cfg = WmConfig { layout: if rng.chance(30) { LayoutKind::Master } else { LayoutKind::Dwindle }, ..Default::default() };
    // Every few seeds, a non-default config.
    if rng.chance(50) {
        cfg.gaps_in = Gaps::all(rng.range(0, 30));
        cfg.gaps_out = Gaps::all(rng.range(0, 60));
        cfg.border_reserve = rng.range(0, 8);
        cfg.workspace_back_and_forth = rng.chance(50);
        cfg.on_focus_under_fullscreen = rng.below(3) as u8;
        cfg.dwindle.preserve_split = rng.chance(50);
        cfg.dwindle.force_split = rng.below(3) as u8;
        cfg.dwindle.use_active_for_splits = rng.chance(50);
        cfg.dwindle.default_split_ratio = 0.2 + rng.below(16) as f64 / 10.0;
        cfg.dwindle.special_scale_factor = 0.5 + rng.below(6) as f64 / 10.0;
        cfg.master.mfact = 0.1 + rng.below(9) as f64 / 10.0;
        cfg.master.new_on_top = rng.chance(50);
        cfg.master.new_status = [NewStatus::Master, NewStatus::Slave, NewStatus::Inherit][rng.below(3) as usize];
        cfg.master.orientation = [Orientation::Left, Orientation::Right, Orientation::Top, Orientation::Bottom, Orientation::Center][rng.below(5) as usize];
    }
    let mut mons = monitor_set(&mut rng);
    if rng.chance(30) {
        for m in &mut mons {
            m.scale = [1.25, 1.5, 1.75, 2.0][rng.below(4) as usize];
        }
    }
    let mut wm = Wm::new(cfg, mons);
    let mut next_id: WinId = 0x1000;
    let mut log: Vec<String> = Vec::new();
    for i in 0..steps {
        let t0 = std::time::Instant::now();
        let op = step(&mut wm, &mut rng, &mut next_id);
        let dt = t0.elapsed();
        if std::env::var_os("STRESS_TIME").is_some() {
            eprintln!("OP {} {} {}", dt.as_micros(), wm.windows.len(), op.split('(').next().unwrap_or(""));
        }
        if std::env::var_os("STRESS_TRACE").is_some() {
            let wins: Vec<String> = wm.windows.values().map(|w| format!("{}@{}{}{}", w.id, w.ws, if w.floating { "f" } else { "" }, if w.minimized { "m" } else { "" })).collect();
            let wss: Vec<String> = wm.workspaces.values().map(|w| format!("{}(m{}):{:?}", w.id, w.monitor, w.layout_windows())).collect();
            let mons: Vec<String> = wm.monitors.iter().map(|m| format!("{}={}/{:?}", m.spec.name, m.active, m.special)).collect();
            eprintln!("[{i}] {op}\n    wins {wins:?}\n    ws {wss:?}\n    mons {mons:?} focus {:?}", wm.focus);
        }
        log.push(op);
        let tail = log.iter().rev().take(8).rev().cloned().collect::<Vec<_>>().join("\n  ");
        let ctx = format!("seed {seed} step {i}; last ops:\n  {tail}\n");
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check(&mut wm, &ctx)));
        if let Err(e) = r {
            std::panic::resume_unwind(e);
        }
    }
}

#[test]
fn random_operations_keep_the_model_consistent() {
    if let Some(seed) = std::env::var("STRESS_SEED").ok().and_then(|s| s.parse().ok()) {
        run(seed, steps());
        return;
    }
    let seeds: u64 = std::env::var("STRESS_SEEDS").ok().and_then(|s| s.parse().ok()).unwrap_or(100);
    for seed in 0..seeds {
        run(seed, steps());
    }
}
