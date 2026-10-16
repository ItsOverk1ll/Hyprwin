//! Hyprland dispatcher strings (`movefocus l`, `workspace e+1`, …) → typed commands.

use hyprwin_layout::{Dir, WsSel};

/// A resize/move parameter: pixels or percent of the monitor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Delta {
    pub value: f64,
    pub percent: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    Exec(String),
    ExecRaw(String),
    KillActive,
    ForceKillActive,
    CloseWindow(String),
    KillWindow(String),
    Workspace(WsSel),
    MoveToWorkspace { ws: WsSel, window: Option<String>, follow: bool },
    ToggleFloating(Option<String>),
    SetFloating(Option<String>),
    SetTiled(Option<String>),
    /// mode 0 = fullscreen, 1 = maximize; `action` toggle/set/unset.
    Fullscreen { mode: u8, action: Option<bool> },
    Pin(Option<String>),
    Pseudo(Option<String>),
    MoveFocus(Dir),
    MoveWindow(Dir),
    MoveWindowToMonitor(String),
    SwapWindow(Dir),
    CenterWindow,
    ResizeActive { x: Delta, y: Delta, exact: bool },
    MoveActive { x: Delta, y: Delta, exact: bool },
    CycleNext { prev: bool, tiled: bool, floating: bool },
    SwapNext { prev: bool },
    FocusWindow(String),
    FocusMonitor(String),
    FocusCurrentOrLast,
    FocusUrgentOrLast,
    ToggleSpecialWorkspace(Option<String>),
    FocusWorkspaceOnCurrentMonitor(WsSel),
    /// `movecurrentworkspacetomonitor` (ws = None) / `moveworkspacetomonitor`.
    MoveWorkspaceToMonitor { ws: Option<WsSel>, monitor: String },
    SwapActiveWorkspaces(String, String),
    RenameWorkspace(i64, String),
    LayoutMsg(String),
    Submap(String),
    BringActiveToTop,
    Exit,
    /// hyprwin extension: pause/resume tiling (windows are restored while paused).
    TogglePause,
    /// hyprwin extension: hide/show the Windows taskbar until the next reload.
    ToggleTaskbar,
    /// hyprwin extension: open a saved App Group by name.
    OpenAppGroup(String),
    Reload,
    /// `bindm` actions.
    MouseMove,
    MouseResize,
    /// Parsed but not available on Windows; the string is the reason.
    Unsupported(String),
}

fn ws_text(w: &WsSel) -> String {
    match w {
        WsSel::Id(n) => format!("workspace {n}"),
        WsSel::Name(n) => format!("workspace \"{n}\""),
        WsSel::Relative(d) | WsSel::MonitorRelative(d) | WsSel::OpenRelative(d) => {
            if *d >= 0 { "the next workspace".into() } else { "the previous workspace".into() }
        }
        WsSel::Previous => "the previous workspace".into(),
        WsSel::Empty => "an empty workspace".into(),
        WsSel::Special(_) => "the scratchpad".into(),
    }
}

fn dir_text(d: Dir) -> &'static str {
    match d {
        Dir::Left => "left",
        Dir::Right => "right",
        Dir::Up => "up",
        Dir::Down => "down",
    }
}

impl Cmd {
    /// Plain-English description for the Settings window's keybind list.
    pub fn describe(&self) -> String {
        match self {
            Cmd::Exec(c) | Cmd::ExecRaw(c) => match c.as_str() {
                "wt" => "Open Windows Terminal".into(),
                "explorer" => "Open File Explorer".into(),
                "explorer shell:AppsFolder" => "Open the apps list".into(),
                other => format!("Run: {other}"),
            },
            Cmd::KillActive | Cmd::CloseWindow(_) => "Close window".into(),
            Cmd::ForceKillActive | Cmd::KillWindow(_) => "Force-quit app".into(),
            Cmd::Workspace(w) => match w {
                WsSel::Special(_) => "Show / hide the scratchpad".into(),
                other => format!("Go to {}", ws_text(other)),
            },
            Cmd::MoveToWorkspace { ws, follow, .. } => format!("Move window to {}{}", ws_text(ws), if *follow { "" } else { " (stay here)" }),
            Cmd::ToggleFloating(_) => "Float / tile window".into(),
            Cmd::SetFloating(_) => "Float window".into(),
            Cmd::SetTiled(_) => "Tile window".into(),
            Cmd::Fullscreen { mode: 1, .. } => "Maximize window".into(),
            Cmd::Fullscreen { .. } => "Fullscreen window".into(),
            Cmd::Pin(_) => "Pin window to all workspaces".into(),
            Cmd::Pseudo(_) => "Pseudotile (keep window size)".into(),
            Cmd::MoveFocus(d) => format!("Focus window {}", dir_text(*d)),
            Cmd::MoveWindow(d) => format!("Move window {}", dir_text(*d)),
            Cmd::MoveWindowToMonitor(m) => format!("Move window to monitor {m}"),
            Cmd::SwapWindow(d) => format!("Swap window {}", dir_text(*d)),
            Cmd::CenterWindow => "Center window".into(),
            Cmd::ResizeActive { x, y, .. } => {
                let dir = if x.value > 0.0 { "wider" } else if x.value < 0.0 { "narrower" } else if y.value > 0.0 { "taller" } else { "shorter" };
                format!("Make window {dir}")
            }
            Cmd::MoveActive { .. } => "Move floating window".into(),
            Cmd::CycleNext { prev, .. } => if *prev { "Focus previous window" } else { "Focus next window" }.into(),
            Cmd::SwapNext { .. } => "Swap with next window".into(),
            Cmd::FocusWindow(_) => "Focus a specific window".into(),
            Cmd::FocusMonitor(m) => format!("Focus monitor {m}"),
            Cmd::FocusCurrentOrLast | Cmd::FocusUrgentOrLast => "Focus the last window".into(),
            Cmd::ToggleSpecialWorkspace(_) => "Show / hide the scratchpad".into(),
            Cmd::FocusWorkspaceOnCurrentMonitor(w) => format!("Bring {} to this monitor", ws_text(w)),
            Cmd::MoveWorkspaceToMonitor { monitor, .. } => format!("Move workspace to monitor {monitor}"),
            Cmd::SwapActiveWorkspaces(..) => "Swap monitors' workspaces".into(),
            Cmd::RenameWorkspace(..) => "Rename workspace".into(),
            Cmd::LayoutMsg(m) => match m.split_whitespace().next().unwrap_or("") {
                "togglesplit" => "Toggle split direction".into(),
                "swapsplit" => "Swap split halves".into(),
                "swapwithmaster" => "Make window the master".into(),
                other => format!("Layout: {other}"),
            },
            Cmd::Submap(s) => format!("Enter mode \"{s}\""),
            Cmd::BringActiveToTop => "Bring window to front".into(),
            Cmd::Exit => "Quit hyprwin".into(),
            Cmd::Reload => "Reload config".into(),
            Cmd::MouseMove => "Drag to move window".into(),
            Cmd::MouseResize => "Drag to resize window".into(),
            Cmd::TogglePause => "Pause / resume tiling".into(),
            Cmd::ToggleTaskbar => "Hide / show the taskbar".into(),
            Cmd::OpenAppGroup(g) => format!("Open app group \"{g}\""),
            Cmd::Unsupported(w) => format!("(not available: {w})"),
        }
    }
}

fn opt(arg: &str) -> Option<String> {
    let a = arg.trim();
    if a.is_empty() || a == "active" { None } else { Some(a.to_string()) }
}

fn parse_delta(s: &str) -> Result<Delta, String> {
    let s = s.trim();
    let (num, percent) = match s.strip_suffix('%') {
        Some(n) => (n, true),
        None => (s, false),
    };
    num.parse::<f64>().map(|value| Delta { value, percent }).map_err(|_| format!("bad number '{s}'"))
}

fn parse_resize(arg: &str) -> Result<(Delta, Delta, bool), String> {
    let mut toks: Vec<&str> = arg.split_whitespace().collect();
    let exact = toks.first() == Some(&"exact");
    if exact {
        toks.remove(0);
    }
    match toks.as_slice() {
        [x, y] => Ok((parse_delta(x)?, parse_delta(y)?, exact)),
        _ => Err(format!("expected 'X Y' got '{arg}'")),
    }
}

fn dir(arg: &str) -> Result<Dir, String> {
    Dir::parse(arg).ok_or_else(|| format!("bad direction '{arg}'"))
}

fn ws(arg: &str) -> Result<WsSel, String> {
    WsSel::parse(arg).ok_or_else(|| format!("bad workspace '{arg}'"))
}

/// Parses `dispatcher` + `arg` (as written after the key in a bind line).
pub fn parse(dispatcher: &str, arg: &str) -> Result<Cmd, String> {
    let d = dispatcher.trim().to_lowercase();
    let a = arg.trim();
    Ok(match d.as_str() {
        "exec" => Cmd::Exec(strip_exec_rules(a)),
        "execr" => Cmd::ExecRaw(a.to_string()),
        "killactive" => Cmd::KillActive,
        "forcekillactive" => Cmd::ForceKillActive,
        "closewindow" => Cmd::CloseWindow(a.to_string()),
        "killwindow" => Cmd::KillWindow(a.to_string()),
        "workspace" => Cmd::Workspace(ws(a)?),
        "movetoworkspace" | "movetoworkspacesilent" => {
            let (w, win) = match a.split_once(',') {
                Some((w, win)) => (w, opt(win)),
                None => (a, None),
            };
            Cmd::MoveToWorkspace { ws: ws(w)?, window: win, follow: d == "movetoworkspace" }
        }
        "togglefloating" => Cmd::ToggleFloating(opt(a)),
        "setfloating" => Cmd::SetFloating(opt(a)),
        "settiled" => Cmd::SetTiled(opt(a)),
        "fullscreen" => {
            let mut t = a.split_whitespace();
            let mode = t.next().and_then(|m| m.parse().ok()).unwrap_or(0);
            let action = match t.next() {
                Some("set") => Some(true),
                Some("unset") => Some(false),
                _ => None,
            };
            Cmd::Fullscreen { mode, action }
        }
        "fullscreenstate" => {
            let internal: i32 = a.split_whitespace().next().and_then(|m| m.parse().ok()).unwrap_or(0);
            match internal {
                0 => Cmd::Fullscreen { mode: 0, action: Some(false) },
                1 => Cmd::Fullscreen { mode: 1, action: Some(true) },
                _ => Cmd::Fullscreen { mode: 0, action: Some(true) },
            }
        }
        "pin" => Cmd::Pin(opt(a)),
        "pseudo" => Cmd::Pseudo(opt(a)),
        "movefocus" => Cmd::MoveFocus(dir(a)?),
        "movewindow" => {
            if a.is_empty() {
                Cmd::MouseMove
            } else if let Some(m) = a.strip_prefix("mon:") {
                Cmd::MoveWindowToMonitor(m.split_whitespace().next().unwrap_or("").to_string())
            } else {
                Cmd::MoveWindow(dir(a)?)
            }
        }
        "resizewindow" => Cmd::MouseResize,
        "swapwindow" => Cmd::SwapWindow(dir(a)?),
        "centerwindow" => Cmd::CenterWindow,
        "resizeactive" => {
            let (x, y, exact) = parse_resize(a)?;
            Cmd::ResizeActive { x, y, exact }
        }
        "moveactive" => {
            let (x, y, exact) = parse_resize(a)?;
            Cmd::MoveActive { x, y, exact }
        }
        "cyclenext" => {
            let has = |w: &str| a.split_whitespace().any(|t| t == w);
            Cmd::CycleNext { prev: has("prev"), tiled: has("tiled"), floating: has("floating") }
        }
        "swapnext" => Cmd::SwapNext { prev: a == "prev" },
        "focuswindow" => Cmd::FocusWindow(a.to_string()),
        "focusmonitor" => Cmd::FocusMonitor(a.to_string()),
        "focuscurrentorlast" => Cmd::FocusCurrentOrLast,
        "focusurgentorlast" => Cmd::FocusUrgentOrLast,
        "togglespecialworkspace" => Cmd::ToggleSpecialWorkspace(opt(a)),
        "focusworkspaceoncurrentmonitor" => Cmd::FocusWorkspaceOnCurrentMonitor(ws(a)?),
        "layoutmsg" => Cmd::LayoutMsg(a.to_string()),
        "submap" => Cmd::Submap(a.to_string()),
        "bringactivetotop" | "alterzorder" => Cmd::BringActiveToTop,
        "exit" => Cmd::Exit,
        "togglepause" | "pause" => Cmd::TogglePause,
        "toggletaskbar" => Cmd::ToggleTaskbar,
        "opengroup" | "openappgroup" => Cmd::OpenAppGroup(a.to_string()),
        "forcerendererreload" => Cmd::Reload,
        "dpms" | "forceidle" => Cmd::Unsupported(format!("{d}: display power is managed by Windows")),
        "togglegroup" | "changegroupactive" | "moveintogroup" | "moveoutofgroup" | "movewindoworgroup" | "lockgroups"
        | "lockactivegroup" | "movegroupwindow" | "denywindowfromgroup" | "setignoregrouplock" => {
            Cmd::Unsupported(format!("{d}: groups are planned for Phase 5"))
        }
        "movecurrentworkspacetomonitor" => Cmd::MoveWorkspaceToMonitor { ws: None, monitor: a.to_string() },
        "moveworkspacetomonitor" => {
            let (w, m) = a.split_once(' ').ok_or_else(|| format!("expected 'WORKSPACE MONITOR', got '{a}'"))?;
            Cmd::MoveWorkspaceToMonitor { ws: Some(ws(w)?), monitor: m.trim().to_string() }
        }
        "swapactiveworkspaces" => {
            let (m1, m2) = a.split_once(' ').ok_or_else(|| format!("expected 'MONITOR1 MONITOR2', got '{a}'"))?;
            Cmd::SwapActiveWorkspaces(m1.to_string(), m2.trim().to_string())
        }
        "renameworkspace" => {
            let (id, name) = a.split_once(' ').unwrap_or((a, ""));
            Cmd::RenameWorkspace(id.trim().parse().map_err(|_| format!("bad workspace id '{id}'"))?, name.trim().to_string())
        }
        "global" | "pass" | "sendshortcut" | "sendkeystate" | "signal" | "signalwindow" | "toggleswallow" | "event" | "setprop"
        | "tagwindow" | "movecursor" | "movecursortocorner" | "resizewindowpixel" | "movewindowpixel" => {
            Cmd::Unsupported(format!("{d}: not implemented yet"))
        }
        _ => return Err(format!("unknown dispatcher '{d}'")),
    })
}

/// `exec [workspace 3 silent] cmd` → `cmd` (exec rules are applied by the daemon later).
fn strip_exec_rules(a: &str) -> String {
    if a.starts_with('[') {
        if let Some(end) = a.find(']') {
            return a[end + 1..].trim().to_string();
        }
    }
    a.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_dispatchers() {
        assert_eq!(parse("exec", "wt").unwrap(), Cmd::Exec("wt".into()));
        assert_eq!(parse("movefocus", "l").unwrap(), Cmd::MoveFocus(Dir::Left));
        assert_eq!(parse("workspace", "e+1").unwrap(), Cmd::Workspace(WsSel::OpenRelative(1)));
        assert_eq!(
            parse("movetoworkspacesilent", "3").unwrap(),
            Cmd::MoveToWorkspace { ws: WsSel::Id(3), window: None, follow: false }
        );
        assert_eq!(parse("togglefloating", "").unwrap(), Cmd::ToggleFloating(None));
        assert_eq!(parse("fullscreen", "1").unwrap(), Cmd::Fullscreen { mode: 1, action: None });
        assert_eq!(parse("movewindow", "").unwrap(), Cmd::MouseMove);
        assert_eq!(parse("togglespecialworkspace", "magic").unwrap(), Cmd::ToggleSpecialWorkspace(Some("magic".into())));
    }

    #[test]
    fn resize_params() {
        assert_eq!(
            parse("resizeactive", "-20 10%").unwrap(),
            Cmd::ResizeActive { x: Delta { value: -20.0, percent: false }, y: Delta { value: 10.0, percent: true }, exact: false }
        );
        assert!(matches!(parse("resizeactive", "exact 800 600").unwrap(), Cmd::ResizeActive { exact: true, .. }));
        assert!(parse("resizeactive", "10").is_err());
    }

    #[test]
    fn exec_rules_are_stripped() {
        assert_eq!(parse("exec", "[workspace 2 silent] wt").unwrap(), Cmd::Exec("wt".into()));
    }

    #[test]
    fn unknown_and_unsupported() {
        assert!(parse("bogus", "").is_err());
        assert!(matches!(parse("togglegroup", "").unwrap(), Cmd::Unsupported(_)));
    }
}
