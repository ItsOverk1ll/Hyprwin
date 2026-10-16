//! Typed configuration built from parsed entries.

use std::collections::HashMap;
use std::path::Path;

use hyprwin_anim::Bezier;
use hyprwin_layout::master::{NewStatus, Orientation};
use hyprwin_layout::wm::WorkspaceRule;
use hyprwin_layout::{Gaps, LayoutKind, WmConfig};

use crate::dispatch::{self, Cmd};
use crate::keys::{Key, Mods};
use crate::parse::{self, Entry, Parsed};
use crate::rules::{self, WindowRule};

/// 0xAARRGGBB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color(pub u32);

impl Color {
    /// `rgba(33ccffee)`, `rgb(33ccff)`, `0xee33ccff`, or a gradient
    /// (`rgba(..) rgba(..) 45deg`), whose first colour is used.
    pub fn parse(s: &str) -> Option<Color> {
        let first = s.split_whitespace().next()?;
        let hex = |h: &str| u32::from_str_radix(h, 16).ok();
        if let Some(h) = first.strip_prefix("rgba(").and_then(|x| x.strip_suffix(')')) {
            let v = hex(h)?;
            return Some(Color((v & 0xFF) << 24 | v >> 8));
        }
        if let Some(h) = first.strip_prefix("rgb(").and_then(|x| x.strip_suffix(')')) {
            return Some(Color(0xFF00_0000 | hex(h)?));
        }
        if let Some(h) = first.strip_prefix("0x") {
            return Some(Color(hex(h)?));
        }
        None
    }
    pub fn rgb(self) -> u32 {
        self.0 & 0x00FF_FFFF
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BindFlags {
    pub locked: bool,
    pub release: bool,
    pub repeat: bool,
    pub mouse: bool,
    pub non_consuming: bool,
    pub transparent: bool,
    pub ignore_mods: bool,
    pub description: bool,
    pub long_press: bool,
    pub click: bool,
    pub drag: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Bind {
    pub mods: Mods,
    pub key: Key,
    pub flags: BindFlags,
    pub cmd: Cmd,
    pub submap: Option<String>,
    pub description: String,
    pub source: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AnimEntry {
    pub enabled: bool,
    pub speed_ds: f64,
    pub curve: String,
    pub style: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub wm: WmConfig,
    pub border_size: i32,
    pub col_active: Color,
    pub col_inactive: Color,
    pub rounding: u32,
    pub follow_mouse: u8,
    pub mouse_refocus: bool,
    pub float_switch_override_focus: u8,
    pub animations_enabled: bool,
    pub beziers: HashMap<String, Bezier>,
    pub animations: HashMap<String, AnimEntry>,
    pub binds: Vec<Bind>,
    pub unbinds: Vec<(Mods, Key)>,
    pub window_rules: Vec<WindowRule>,
    pub exec_once: Vec<String>,
    pub exec: Vec<String>,
    pub exec_shutdown: Vec<String>,
    pub env: Vec<(String, String)>,
    /// `monitor = NAME, addreserved, top, bottom, left, right`
    pub reserved: HashMap<String, (i32, i32, i32, i32)>,
    // hyprwin-specific (section `hyprwin { }`)
    pub hide_taskbar: bool,
    /// `hyprwin:force_tile`: only real dialogs float automatically; windows
    /// that are always-on-top, fixed-size or borderless popups tile too.
    pub force_tile: bool,
    pub warnings: Vec<String>,
    pub files: Vec<std::path::PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        let mut beziers = HashMap::new();
        beziers.insert("default".to_string(), Bezier::HYPR_DEFAULT);
        beziers.insert("linear".to_string(), Bezier::LINEAR);
        Config {
            wm: WmConfig::default(),
            border_size: 1,
            col_active: Color(0xEE33CCFF),
            col_inactive: Color(0xAA595959),
            rounding: 0,
            follow_mouse: 1,
            mouse_refocus: true,
            float_switch_override_focus: 1,
            animations_enabled: true,
            beziers,
            animations: HashMap::new(),
            binds: Vec::new(),
            unbinds: Vec::new(),
            window_rules: Vec::new(),
            exec_once: Vec::new(),
            exec: Vec::new(),
            exec_shutdown: Vec::new(),
            env: Vec::new(),
            reserved: HashMap::new(),
            hide_taskbar: false,
            force_tile: false,
            warnings: Vec::new(),
            files: Vec::new(),
        }
    }
}

fn b(v: &str) -> Option<bool> {
    match v.trim().to_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// Keys that are Wayland/compositor-only and silently have no effect on Windows.
/// They are still reported once, grouped, so users know.
const IGNORED_PREFIXES: &[&str] = &[
    "decoration:blur",
    "decoration:shadow",
    "decoration:dim",
    "decoration:screen_shader",
    "decoration:fullscreen_opacity",
    "input:kb_",
    "input:numlock_by_default",
    "input:repeat_",
    "input:sensitivity",
    "input:accel_profile",
    "input:natural_scroll",
    "input:touchpad",
    "input:touchdevice",
    "input:tablet",
    "gestures:",
    "misc:disable_hyprland_logo",
    "misc:disable_splash_rendering",
    "misc:force_default_wallpaper",
    "misc:vfr",
    "misc:vrr",
    "xwayland:",
    "opengl:",
    "render:",
    "cursor:",
    "ecosystem:",
    "debug:",
    "device:",
    "experimental:",
    "plugin:",
    "group:",
];

impl Config {
    pub fn load_file(path: &Path) -> Config {
        let parsed = parse::parse_file(path);
        Self::from_parsed(parsed)
    }

    pub fn load_str(text: &str) -> Config {
        Self::from_parsed(parse::parse_str(text, "<string>"))
    }

    pub fn from_parsed(p: Parsed) -> Config {
        let mut c = Config { warnings: p.warnings, files: p.files, ..Default::default() };
        let mut submap: Option<String> = None;
        let mut ignored: Vec<String> = Vec::new();
        for e in &p.entries {
            if let Err(w) = c.apply(e, &mut submap, &mut ignored) {
                c.warnings.push(format!("{}:{}: {w}", e.file, e.line));
            }
        }
        if !ignored.is_empty() {
            ignored.sort();
            ignored.dedup();
            c.warnings.push(format!("ignored (no Windows equivalent): {}", ignored.join(", ")));
        }
        c
    }

    fn apply(&mut self, e: &Entry, submap: &mut Option<String>, ignored: &mut Vec<String>) -> Result<(), String> {
        let v = e.value.as_str();
        let src = format!("{}:{}", e.file, e.line);
        let int = |v: &str| v.trim().parse::<i32>().map_err(|_| format!("{} expects an integer, got '{v}'", e.key));
        let float = |v: &str| v.trim().parse::<f64>().map_err(|_| format!("{} expects a number, got '{v}'", e.key));
        let boolean = |v: &str| b(v).ok_or_else(|| format!("{} expects a bool, got '{v}'", e.key));
        let gaps = |v: &str| Gaps::parse(v).ok_or_else(|| format!("{} expects gaps, got '{v}'", e.key));
        let color = |v: &str| Color::parse(v).ok_or_else(|| format!("{} expects a colour, got '{v}'", e.key));

        if e.key.starts_with("bind") {
            return self.add_bind(&e.key, v, submap.clone(), &src);
        }
        match e.key.as_str() {
            "general:gaps_in" => self.wm.gaps_in = gaps(v)?,
            "general:gaps_out" => self.wm.gaps_out = gaps(v)?,
            "general:border_size" => self.border_size = int(v)?,
            "general:col.active_border" => {
                self.col_active = color(v)?;
                if v.split_whitespace().count() > 1 {
                    self.warnings.push(format!("{src}: gradient borders are drawn with their first colour (DWM borders are solid)"));
                }
            }
            "general:col.inactive_border" => self.col_inactive = color(v)?,
            "general:layout" => {
                self.wm.layout = match v.trim() {
                    "dwindle" => LayoutKind::Dwindle,
                    "master" => LayoutKind::Master,
                    other => return Err(format!("layout '{other}' is not supported (dwindle, master)")),
                }
            }
            "general:resize_on_border" | "general:extend_border_grab_area" | "general:hover_icon_on_border" | "general:allow_tearing"
            | "general:col.nogroup_border" | "general:col.nogroup_border_active" | "general:no_focus_fallback" | "general:snap:enabled" => {}
            "decoration:rounding" => self.rounding = int(v)?.max(0) as u32,
            "decoration:rounding_power" | "decoration:active_opacity" | "decoration:inactive_opacity" => ignored.push(e.key.clone()),
            "input:follow_mouse" => self.follow_mouse = int(v)?.clamp(0, 3) as u8,
            "input:mouse_refocus" => self.mouse_refocus = boolean(v)?,
            "input:float_switch_override_focus" => self.float_switch_override_focus = int(v)?.clamp(0, 2) as u8,
            "dwindle:force_split" => self.wm.dwindle.force_split = int(v)?.clamp(0, 2) as u8,
            "dwindle:preserve_split" => self.wm.dwindle.preserve_split = boolean(v)?,
            "dwindle:split_width_multiplier" => self.wm.dwindle.split_width_multiplier = float(v)?,
            "dwindle:default_split_ratio" => self.wm.dwindle.default_split_ratio = float(v)?,
            "dwindle:use_active_for_splits" => self.wm.dwindle.use_active_for_splits = boolean(v)?,
            "dwindle:permanent_direction_override" => self.wm.dwindle.permanent_direction_override = boolean(v)?,
            "dwindle:special_scale_factor" | "master:special_scale_factor" => self.wm.dwindle.special_scale_factor = float(v)?,
            "dwindle:pseudotile" | "dwindle:smart_split" | "dwindle:smart_resizing" | "dwindle:split_bias" | "dwindle:precise_mouse_move" => {}
            "master:mfact" => self.wm.master.mfact = float(v)?,
            "master:new_status" => {
                self.wm.master.new_status = match v.trim() {
                    "master" => NewStatus::Master,
                    "inherit" => NewStatus::Inherit,
                    _ => NewStatus::Slave,
                }
            }
            "master:new_on_top" => self.wm.master.new_on_top = boolean(v)?,
            "master:orientation" => self.wm.master.orientation = Orientation::parse(v.trim()).ok_or("bad orientation")?,
            "master:slave_count_for_center_master" => self.wm.master.slave_count_for_center_master = int(v)?.max(0) as usize,
            "master:allow_small_split" => self.wm.master.allow_small_split = boolean(v)?,
            "master:new_on_active" | "master:smart_resizing" | "master:drop_at_cursor" | "master:center_master_fallback"
            | "master:always_keep_position" | "master:new_is_master" => {}
            "binds:workspace_back_and_forth" => self.wm.workspace_back_and_forth = boolean(v)?,
            "binds:allow_workspace_cycles" | "binds:drag_threshold" | "binds:scroll_event_delay" => {}
            "misc:on_focus_under_fullscreen" => self.wm.on_focus_under_fullscreen = int(v)?.clamp(0, 2) as u8,
            "misc:new_window_takes_over_fullscreen" => self.wm.on_focus_under_fullscreen = int(v)?.clamp(0, 2) as u8,
            "animations:enabled" => self.animations_enabled = boolean(v)?,
            "animations:first_launch_animation" | "animations:workspace_wraparound" => {}
            "bezier" => {
                let p: Vec<&str> = v.split(',').map(str::trim).collect();
                if p.len() != 5 {
                    return Err(format!("bezier expects NAME, X0, Y0, X1, Y1: '{v}'"));
                }
                let n: Vec<f64> = p[1..].iter().map(|x| x.parse::<f64>()).collect::<Result<_, _>>().map_err(|_| format!("bad bezier '{v}'"))?;
                self.beziers.insert(p[0].to_string(), Bezier::new(n[0], n[1], n[2], n[3]));
            }
            "animation" => {
                let p: Vec<&str> = v.split(',').map(str::trim).collect();
                let name = p.first().copied().unwrap_or("").to_string();
                let enabled = p.get(1).and_then(|x| b(x)).unwrap_or(false);
                let entry = AnimEntry {
                    enabled,
                    speed_ds: p.get(2).and_then(|x| x.parse().ok()).unwrap_or(5.0),
                    curve: p.get(3).map(|s| s.to_string()).unwrap_or_else(|| "default".into()),
                    style: p.get(4).map(|s| s.to_string()),
                };
                if enabled && !self.beziers.contains_key(&entry.curve) {
                    self.warnings.push(format!("{src}: unknown bezier '{}' (defined later?)", entry.curve));
                }
                self.animations.insert(name, entry);
            }
            "windowrulev2" => self.add_rule(rules::parse_v2(v, &src)),
            "windowrule" if e.block.is_empty() => self.add_rule(rules::parse_windowrule(v, &src)),
            "windowrule" => self.add_rule(rules::parse_block(&e.block, &src)),
            "workspace" => self.add_workspace_rule(v, &e.block)?,
            "exec-once" => self.exec_once.push(v.to_string()),
            "exec" => self.exec.push(v.to_string()),
            "exec-shutdown" => self.exec_shutdown.push(v.to_string()),
            "env" => {
                let (k, val) = v.split_once(',').ok_or("env expects NAME, value")?;
                self.env.push((k.trim().to_string(), val.trim().to_string()));
            }
            "submap" => *submap = if v.trim() == "reset" { None } else { Some(v.trim().to_string()) },
            "unbind" => {
                let (m, k) = v.split_once(',').ok_or("unbind expects MODS, key")?;
                self.unbinds.push((Mods::parse(m)?, Key::parse(k)?));
            }
            "monitor" => {
                let p: Vec<&str> = v.split(',').map(str::trim).collect();
                if p.get(1) == Some(&"addreserved") && p.len() >= 6 {
                    let n: Vec<i32> = p[2..6].iter().filter_map(|x| x.parse().ok()).collect();
                    if n.len() == 4 {
                        self.reserved.insert(p[0].to_string(), (n[0], n[1], n[2], n[3]));
                    }
                } else {
                    ignored.push("monitor (resolution/position/scale are set in Windows Display settings)".into());
                }
            }
            "layerrule" | "permission" | "plugin" | "gesture" => ignored.push(e.key.clone()),
            "hyprwin:hide_taskbar" => self.hide_taskbar = boolean(v)?,
            "hyprwin:force_tile" => self.force_tile = boolean(v)?,
            k if IGNORED_PREFIXES.iter().any(|p| k.starts_with(p)) => ignored.push(k.to_string()),
            k => return Err(format!("unknown option '{k}'")),
        }
        Ok(())
    }

    fn add_rule(&mut self, r: rules::ParsedRule) {
        self.warnings.extend(r.warnings);
        if let Some(rule) = r.rule {
            self.window_rules.push(rule);
        }
    }

    fn add_workspace_rule(&mut self, v: &str, _block: &[(String, String)]) -> Result<(), String> {
        let mut parts = v.split(',').map(str::trim);
        let sel = parts.next().ok_or("workspace rule needs a workspace")?;
        let mut rule = WorkspaceRule { id: None, name: None, monitor: None, default: false, persistent: false, gaps_in: None, gaps_out: None, layout: None };
        if let Ok(n) = sel.parse::<i64>() {
            rule.id = Some(n);
        } else if let Some(n) = sel.strip_prefix("name:") {
            rule.name = Some(n.to_string());
        } else if sel.starts_with("special") {
            rule.name = Some(sel.to_string());
        } else {
            return Err(format!("workspace rule selector '{sel}' is not supported yet"));
        }
        for p in parts {
            let Some((k, val)) = p.split_once(':') else { continue };
            match k.trim() {
                "monitor" => rule.monitor = Some(normalize_monitor(val)),
                "default" => rule.default = b(val).unwrap_or(false),
                "persistent" => rule.persistent = b(val).unwrap_or(false),
                "gapsin" => rule.gaps_in = Gaps::parse(val),
                "gapsout" => rule.gaps_out = Gaps::parse(val),
                "layout" => rule.layout = if val.trim() == "master" { Some(LayoutKind::Master) } else { Some(LayoutKind::Dwindle) },
                other => self.warnings.push(format!("workspace rule '{other}' not supported yet")),
            }
        }
        self.wm.workspace_rules.push(rule);
        Ok(())
    }

    fn add_bind(&mut self, keyword: &str, v: &str, submap: Option<String>, src: &str) -> Result<(), String> {
        let flags_s = &keyword["bind".len()..];
        let mut flags = BindFlags::default();
        for c in flags_s.chars() {
            match c {
                'l' => flags.locked = true,
                'r' => flags.release = true,
                'e' => flags.repeat = true,
                'm' => flags.mouse = true,
                'n' => flags.non_consuming = true,
                't' => flags.transparent = true,
                'i' => flags.ignore_mods = true,
                'd' => flags.description = true,
                'o' => flags.long_press = true,
                'c' => flags.click = true,
                'g' => flags.drag = true,
                's' | 'p' | 'u' | 'k' => {}
                other => return Err(format!("unknown bind flag '{other}'")),
            }
        }
        // MODS, key[, description], dispatcher[, args...]
        let mut parts = v.splitn(if flags.description { 5 } else { 4 }, ',').map(str::trim);
        let mods = Mods::parse(parts.next().unwrap_or(""))?;
        let key = Key::parse(parts.next().ok_or("bind needs a key")?)?;
        let description = if flags.description { parts.next().unwrap_or("").to_string() } else { String::new() };
        let disp = parts.next().ok_or("bind needs a dispatcher")?;
        let arg = parts.next().unwrap_or("");
        let cmd = dispatch::parse(disp, arg)?;
        if let Cmd::Unsupported(why) = &cmd {
            self.warnings.push(format!("{src}: {why}"));
        }
        if key.is_mouse() && matches!(key, Key::MouseLeft | Key::MouseRight) && mods == Mods::NONE && !flags.mouse {
            return Err("refusing to bind a bare mouse button (it would make the mouse unusable)".into());
        }
        self.binds.push(Bind { mods, key, flags, cmd, submap, description, source: src.to_string() });
        Ok(())
    }

    /// Final bind list after `unbind`s.
    pub fn effective_binds(&self) -> Vec<Bind> {
        self.binds.iter().filter(|b| !self.unbinds.contains(&(b.mods, b.key))).cloned().collect()
    }
}

/// Accepts `DISPLAY1`, `\\.\DISPLAY1`, or a Hyprland-style name, and returns the
/// form hyprwin uses (`DISPLAY1`).
pub fn normalize_monitor(s: &str) -> String {
    s.trim().trim_start_matches(r"\\.\").to_string()
}

/// A resolved `animation =` leaf: how long, which curve, which style.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedAnim {
    pub duration_ms: f64,
    pub curve: Bezier,
    pub style: Option<String>,
}

/// Hyprland's animation tree: a leaf that is not set inherits from its parent.
fn anim_parent(name: &str) -> Option<&'static str> {
    Some(match name {
        "windowsIn" | "windowsOut" | "windowsMove" => "windows",
        "workspacesIn" | "workspacesOut" | "specialWorkspace" => "workspaces",
        "specialWorkspaceIn" | "specialWorkspaceOut" => "specialWorkspace",
        "fadeIn" | "fadeOut" | "fadeSwitch" | "fadeShadow" | "fadeDim" | "fadeLayers" | "fadePopups" | "fadeDpms" => "fade",
        "borderangle" => "border",
        "windows" | "workspaces" | "fade" | "border" | "layers" | "zoomFactor" => "global",
        _ => return None,
    })
}

impl Config {
    /// The animation to use for `name` (e.g. `windowsMove`), or `None` when
    /// animations are off, the node (or the ancestor it inherits from) is
    /// disabled, or its speed is 0.
    pub fn anim(&self, name: &str) -> Option<ResolvedAnim> {
        if !self.animations_enabled {
            return None;
        }
        let mut node = name;
        let entry = loop {
            if let Some(e) = self.animations.get(node) {
                break e.clone();
            }
            match anim_parent(node) {
                Some(p) => node = p,
                // Hyprland's built-in global: on, speed 8, default curve.
                None => break AnimEntry { enabled: true, speed_ds: 8.0, curve: "default".into(), style: None },
            }
        };
        if !entry.enabled || entry.speed_ds <= 0.0 {
            return None;
        }
        // A style set higher up still applies to children that don't set one.
        let style = entry.style.clone().or_else(|| {
            let mut n = node;
            while let Some(p) = anim_parent(n) {
                if let Some(s) = self.animations.get(p).and_then(|e| e.style.clone()) {
                    return Some(s);
                }
                n = p;
            }
            None
        });
        Some(ResolvedAnim {
            duration_ms: hyprwin_anim::speed_to_ms(entry.speed_ds),
            curve: self.beziers.get(&entry.curve).copied().unwrap_or(Bezier::HYPR_DEFAULT),
            style,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_loads_cleanly() {
        let c = Config::load_str(crate::DEFAULT_CONFIG);
        assert!(c.warnings.is_empty(), "{:#?}", c.warnings);
        assert!(c.binds.len() > 40);
        assert!(c.binds.iter().any(|b| b.flags.mouse && b.cmd == Cmd::MouseMove));
    }

    #[test]
    fn colours() {
        assert_eq!(Color::parse("rgba(33ccffee)"), Some(Color(0xEE33CCFF)));
        assert_eq!(Color::parse("rgb(ff0000)"), Some(Color(0xFFFF0000)));
        assert_eq!(Color::parse("0xff00ff00"), Some(Color(0xFF00FF00)));
        assert_eq!(Color::parse("rgba(33ccffee) rgba(00ff99ee) 45deg").unwrap().rgb(), 0x33CCFF);
    }

    #[test]
    fn real_world_snippet_with_unsupported_bits() {
        let c = Config::load_str(
            r#"
$mainMod = SUPER
general {
    gaps_in = 3
    gaps_out = 8
    col.active_border = rgba(33ccffee) rgba(00ff99ee) 45deg
    layout = master
}
decoration {
    rounding = 10
    blur {
        enabled = true
        size = 3
    }
}
input {
    kb_layout = us
    follow_mouse = 1
    touchpad {
        natural_scroll = false
    }
}
animations {
    enabled = yes
    bezier = myBezier, 0.05, 0.9, 0.1, 1.05
    animation = windows, 1, 7, myBezier
    animation = workspaces, 1, 6, default, slidevert
}
master {
    new_status = master
}
windowrulev2 = suppressevent maximize, class:.*
windowrulev2 = float, class:^(pavucontrol)$
bind = $mainMod, Q, exec, kitty
bindel = ,XF86AudioRaiseVolume, exec, wpctl set-volume @DEFAULT_AUDIO_SINK@ 5%+
bind = $mainMod, mouse_down, workspace, e+1
bindm = $mainMod, mouse:272, movewindow
monitor=,preferred,auto,auto
"#,
        );
        assert_eq!(c.wm.gaps_in, Gaps::all(3));
        assert_eq!(c.wm.layout, LayoutKind::Master);
        assert_eq!(c.rounding, 10);
        assert_eq!(c.binds.len(), 4);
        assert_eq!(c.window_rules.len(), 1);
        assert_eq!(c.animations["workspaces"].style.as_deref(), Some("slidevert"));
        assert!(c.beziers.contains_key("myBezier"));
        // gradient + suppressevent + ignored group
        assert!(c.warnings.iter().any(|w| w.contains("gradient")));
        assert!(c.warnings.iter().any(|w| w.contains("suppressevent")));
        assert!(c.warnings.iter().any(|w| w.contains("decoration:blur")));
    }

    #[test]
    fn bind_with_description_and_unbind() {
        let c = Config::load_str("bindd = SUPER, Q, Open terminal, exec, wt\nunbind = SUPER, Q\n");
        assert_eq!(c.binds[0].description, "Open terminal");
        assert!(c.effective_binds().is_empty());
    }

    #[test]
    fn submaps() {
        let c = Config::load_str("bind = ALT, R, submap, resize\nsubmap = resize\nbinde = , right, resizeactive, 10 0\nbind = , escape, submap, reset\nsubmap = reset\n");
        assert_eq!(c.binds[1].submap.as_deref(), Some("resize"));
        assert_eq!(c.binds[0].submap, None);
    }

    #[test]
    fn workspace_rules() {
        let c = Config::load_str("workspace = 1, monitor:DISPLAY2, default:true, persistent:true\nworkspace = 3, gapsout:0, layout:master\n");
        assert_eq!(c.wm.workspace_rules.len(), 2);
        assert_eq!(c.wm.workspace_rules[0].monitor.as_deref(), Some("DISPLAY2"));
        assert_eq!(c.wm.workspace_rules[1].layout, Some(LayoutKind::Master));
    }
    #[test]
    fn animation_tree_inherits_and_disables() {
        let c = Config::load_str("animations {
 bezier = q, 0.1, 0.9, 0.1, 1
 animation = windows, 1, 4, q, slide
 animation = windowsIn, 1, 2, q, popin 80%
 animation = workspaces, 0, 5, default
}
");
        let m = c.anim("windowsMove").unwrap();
        assert_eq!(m.duration_ms, 400.0);
        assert_eq!(m.style.as_deref(), Some("slide"));
        assert_eq!(c.anim("windowsIn").unwrap().style.as_deref(), Some("popin 80%"));
        assert!(c.anim("workspacesIn").is_none(), "parent disabled");
        assert_eq!(c.anim("fadeIn").unwrap().duration_ms, 800.0, "global default");
        let off = Config::load_str("animations {
 enabled = false
}
");
        assert!(off.anim("windowsMove").is_none());
    }
}
