//! `binds.conf`: keybind changes made in the Settings window.
//!
//! It is sourced after everything else (see `themes::managed_block`), so it
//! only needs Hyprland's own syntax: `unbind = MODS, key` drops a bind from
//! the user's config, and `bind = …` lines add new ones. The user's
//! hyprland.conf is never edited.

use crate::keys::{Key, Mods};
use crate::paths;

#[derive(Clone, Debug, PartialEq)]
pub struct BindLine {
    /// Flag letters after `bind` (e.g. `e` for repeat, `m` for mouse).
    pub flags: String,
    pub mods: Mods,
    pub key: Key,
    pub dispatcher: String,
    pub arg: String,
}

impl BindLine {
    pub fn combo(&self) -> (Mods, Key) {
        (self.mods, self.key)
    }

    pub fn render(&self) -> String {
        let esc = crate::parse::escape_value;
        let arg = if self.arg.is_empty() { String::new() } else { format!(" {}", esc(&self.arg)) };
        format!("bind{} = {}, {}, {},{}", esc(&self.flags), self.mods, self.key.name(), esc(&self.dispatcher), arg)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BindsFile {
    /// Keys whose binding from the main config is removed or replaced.
    pub unbinds: Vec<(Mods, Key)>,
    pub binds: Vec<BindLine>,
}

impl BindsFile {
    pub fn load() -> BindsFile {
        Self::parse(&std::fs::read_to_string(paths::binds_conf()).unwrap_or_default())
    }

    pub fn parse(text: &str) -> BindsFile {
        let mut f = BindsFile::default();
        for e in crate::parse::parse_str(text, "binds.conf").entries {
            let v = e.value.as_str();
            if e.key == "unbind" {
                if let Some((m, k)) = v.split_once(',') {
                    if let (Ok(m), Ok(k)) = (Mods::parse(m), Key::parse(k)) {
                        f.unbinds.push((m, k));
                    }
                }
            } else if let Some(flags) = e.key.strip_prefix("bind") {
                let mut p = v.splitn(4, ',').map(str::trim);
                let (Some(m), Some(k), Some(d)) = (p.next(), p.next(), p.next()) else { continue };
                let (Ok(mods), Ok(key)) = (Mods::parse(m), Key::parse(k)) else { continue };
                f.binds.push(BindLine { flags: flags.to_string(), mods, key, dispatcher: d.to_string(), arg: p.next().unwrap_or("").to_string() });
            }
        }
        f
    }

    pub fn render(&self) -> String {
        let mut out = String::from("# Keybind changes made in hyprwin's Settings window (Keybinds tab).\n# Sourced after your config. Safe to edit or delete.\n");
        for (m, k) in &self.unbinds {
            out.push_str(&format!("unbind = {}, {}\n", m, k.name()));
        }
        for b in &self.binds {
            out.push_str(&b.render());
            out.push('\n');
        }
        out
    }

    pub fn save(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(paths::app_dir())?;
        let tmp = paths::binds_conf().with_extension("conf.tmp");
        std::fs::write(&tmp, self.render())?;
        std::fs::rename(&tmp, paths::binds_conf())
    }

    fn ours(&self, c: (Mods, Key)) -> Option<usize> {
        self.binds.iter().position(|b| b.combo() == c)
    }

    fn unbind(&mut self, c: (Mods, Key)) {
        if !self.unbinds.contains(&c) {
            self.unbinds.push(c);
        }
    }

    /// Removes the bind on `c`, wherever it was defined.
    pub fn remove(&mut self, c: (Mods, Key)) {
        match self.ours(c) {
            Some(i) => {
                self.binds.remove(i);
            }
            None => self.unbind(c),
        }
    }

    /// Replaces the bind on `original` (None = a new bind) with `new`. Whatever
    /// the main config bound on `new`'s keys is dropped, so keys never do two
    /// things at once.
    pub fn set(&mut self, original: Option<(Mods, Key)>, new: BindLine) {
        if let Some(o) = original {
            if o != new.combo() {
                self.remove(o);
            }
        }
        let c = new.combo();
        self.unbind(c);
        match self.ours(c) {
            Some(i) => self.binds[i] = new,
            None => self.binds.push(new),
        }
    }
}

// ------------------------------------------------------------------ action catalog

/// What an action needs besides its dispatcher.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgKind {
    /// Fixed argument (may be empty).
    Fixed(&'static str),
    /// A workspace number 1–10.
    Workspace,
    /// left / right / up / down → l / r / u / d
    Direction,
    /// Free text, with a hint.
    Text(&'static str),
}

pub struct Action {
    pub label: &'static str,
    pub dispatcher: &'static str,
    pub arg: ArgKind,
}

/// The actions offered in the keybind editor, in plain English.
pub const ACTIONS: &[Action] = &[
    Action { label: "Open a program", dispatcher: "exec", arg: ArgKind::Text("command, e.g. wt or notepad") },
    Action { label: "Close window", dispatcher: "killactive", arg: ArgKind::Fixed("") },
    Action { label: "Force-quit app", dispatcher: "forcekillactive", arg: ArgKind::Fixed("") },
    Action { label: "Float / tile window", dispatcher: "togglefloating", arg: ArgKind::Fixed("") },
    Action { label: "Fullscreen", dispatcher: "fullscreen", arg: ArgKind::Fixed("0") },
    Action { label: "Maximize", dispatcher: "fullscreen", arg: ArgKind::Fixed("1") },
    Action { label: "Pin window to all workspaces", dispatcher: "pin", arg: ArgKind::Fixed("") },
    Action { label: "Pseudotile (keep window size)", dispatcher: "pseudo", arg: ArgKind::Fixed("") },
    Action { label: "Center window", dispatcher: "centerwindow", arg: ArgKind::Fixed("") },
    Action { label: "Toggle split direction", dispatcher: "layoutmsg", arg: ArgKind::Fixed("togglesplit") },
    Action { label: "Focus window in a direction", dispatcher: "movefocus", arg: ArgKind::Direction },
    Action { label: "Move window in a direction", dispatcher: "movewindow", arg: ArgKind::Direction },
    Action { label: "Swap window in a direction", dispatcher: "swapwindow", arg: ArgKind::Direction },
    Action { label: "Go to workspace", dispatcher: "workspace", arg: ArgKind::Workspace },
    Action { label: "Move window to workspace", dispatcher: "movetoworkspace", arg: ArgKind::Workspace },
    Action { label: "Move window to workspace (stay here)", dispatcher: "movetoworkspacesilent", arg: ArgKind::Workspace },
    Action { label: "Next workspace", dispatcher: "workspace", arg: ArgKind::Fixed("e+1") },
    Action { label: "Previous workspace", dispatcher: "workspace", arg: ArgKind::Fixed("e-1") },
    Action { label: "Show / hide the scratchpad", dispatcher: "togglespecialworkspace", arg: ArgKind::Fixed("magic") },
    Action { label: "Send window to the scratchpad", dispatcher: "movetoworkspace", arg: ArgKind::Fixed("special:magic") },
    Action { label: "Open an app group", dispatcher: "opengroup", arg: ArgKind::Text("group name") },
    Action { label: "Pause / resume tiling", dispatcher: "togglepause", arg: ArgKind::Fixed("") },
    Action { label: "Hide / show the taskbar", dispatcher: "toggletaskbar", arg: ArgKind::Fixed("") },
    Action { label: "Quit hyprwin", dispatcher: "exit", arg: ArgKind::Fixed("") },
];

pub const DIRECTIONS: &[(&str, &str)] = &[("left", "l"), ("right", "r"), ("up", "u"), ("down", "d")];

/// Finds the catalog entry and argument value for an existing bind; `None`
/// means it is only editable as a custom dispatcher.
pub fn match_action(dispatcher: &str, arg: &str) -> Option<(usize, String)> {
    let arg = arg.trim();
    ACTIONS.iter().enumerate().find_map(|(i, a)| {
        if !a.dispatcher.eq_ignore_ascii_case(dispatcher.trim()) {
            return None;
        }
        match a.arg {
            ArgKind::Fixed(f) if f.eq_ignore_ascii_case(arg) => Some((i, String::new())),
            ArgKind::Fixed(_) => None,
            ArgKind::Workspace => arg.parse::<u32>().ok().filter(|n| (1..=10).contains(n)).map(|n| (i, n.to_string())),
            // Any spelling the daemon accepts: l / left, u / up / t / top, …
            ArgKind::Direction => {
                let short = match arg.to_ascii_lowercase().as_str() {
                    "l" | "left" => "l",
                    "r" | "right" => "r",
                    "u" | "up" | "t" | "top" => "u",
                    "d" | "down" | "b" | "bottom" => "d",
                    _ => return None,
                };
                DIRECTIONS.iter().find(|(_, d)| *d == short).map(|(_, d)| (i, d.to_string()))
            }
            ArgKind::Text(_) => Some((i, arg.to_string())),
        }
    })
}

/// Keys offered in the editor, as (hyprland.conf name, label).
pub fn key_choices() -> Vec<String> {
    let mut v: Vec<String> = ('A'..='Z').map(|c| c.to_string()).collect();
    v.extend(('0'..='9').map(|c| c.to_string()));
    v.extend((1..=12).map(|n| format!("F{n}")));
    for n in ["left", "right", "up", "down", "Return", "space", "Tab", "Escape", "BackSpace", "Delete", "Insert", "Home", "End", "Prior", "Next", "Print", "minus", "equal", "comma", "period", "slash", "semicolon", "apostrophe", "grave", "bracketleft", "bracketright", "backslash"] {
        v.push(n.to_string());
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Config;

    fn line(m: &str, k: &str, d: &str, a: &str) -> BindLine {
        BindLine { flags: String::new(), mods: Mods::parse(m).unwrap(), key: Key::parse(k).unwrap(), dispatcher: d.into(), arg: a.into() }
    }

    fn effective(base: &str, f: &BindsFile) -> Vec<(String, String, String)> {
        let c = Config::load_str(&format!("{base}\n{}", f.render()));
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
        c.effective_binds().into_iter().map(|b| (format!("{}+{}", b.mods, b.key.name()), b.dispatcher, b.arg)).collect()
    }

    const BASE: &str = "bind = SUPER, Q, exec, wt\nbind = SUPER, C, killactive,\n";

    #[test]
    fn change_the_keys_of_a_default_bind() {
        let mut f = BindsFile::default();
        f.set(Some((Mods::SUPER, Key::parse("Q").unwrap())), line("SUPER", "Return", "exec", "wt"));
        let e = effective(BASE, &f);
        assert!(e.contains(&("SUPER+Return".into(), "exec".into(), "wt".into())));
        assert!(!e.iter().any(|b| b.0 == "SUPER+Q"), "old keys freed: {e:?}");
    }

    #[test]
    fn rebinding_keys_that_were_taken_replaces_the_old_action() {
        let mut f = BindsFile::default();
        f.set(None, line("SUPER", "C", "exec", "notepad"));
        let e = effective(BASE, &f);
        assert_eq!(e.iter().filter(|b| b.0 == "SUPER+C").count(), 1);
        assert!(e.contains(&("SUPER+C".into(), "exec".into(), "notepad".into())));
    }

    #[test]
    fn remove_default_and_own_binds_and_round_trip() {
        let mut f = BindsFile::default();
        f.set(None, line("SUPER SHIFT", "N", "exec", "notepad"));
        f.remove((Mods::SUPER, Key::parse("C").unwrap()));
        assert_eq!(BindsFile::parse(&f.render()), f);
        let e = effective(BASE, &f);
        assert!(!e.iter().any(|b| b.0 == "SUPER+C"));
        assert!(e.iter().any(|b| b.0 == "SUPER_SHIFT+N"));
        f.remove((Mods(Mods::SUPER.0 | Mods::SHIFT.0), Key::parse("N").unwrap()));
        assert!(f.binds.is_empty(), "removing our own bind drops its line");
    }

    #[test]
    fn catalog_matches_existing_binds() {
        assert_eq!(match_action("movefocus", "l").map(|m| m.1), Some("l".into()));
        assert_eq!(match_action("workspace", "3").map(|m| ACTIONS[m.0].label), Some("Go to workspace"));
        assert_eq!(match_action("workspace", "e+1").map(|m| ACTIONS[m.0].label), Some("Next workspace"));
        assert_eq!(match_action("fullscreen", "1").map(|m| ACTIONS[m.0].label), Some("Maximize"));
        assert_eq!(match_action("exec", "wt").map(|m| m.1), Some("wt".into()));
        assert!(match_action("resizeactive", "10 0").is_none());
        // Long direction names are the same action as the short ones.
        assert_eq!(match_action("movefocus", "left"), match_action("movefocus", "l"));
        assert_eq!(match_action("movewindow", "Down").map(|m| m.1), Some("d".into()));
        assert!(match_action("movefocus", "sideways").is_none());
        for a in ACTIONS {
            let arg = match a.arg {
                ArgKind::Fixed(f) => f.to_string(),
                ArgKind::Workspace => "2".into(),
                ArgKind::Direction => "l".into(),
                ArgKind::Text(_) => "x".into(),
            };
            let c = Config::load_str(&format!("bind = SUPER, K, {}, {}\n", a.dispatcher, arg));
            assert!(c.warnings.is_empty() && c.binds.len() == 1, "{}: {:?}", a.label, c.warnings);
        }
        for k in key_choices() {
            assert!(Key::parse(&k).is_ok(), "{k}");
        }
    }

    #[test]
    fn hashes_and_line_breaks_survive_a_save() {
        let mut f = BindsFile::default();
        let arg = "start https://example.com/#top && C:\\Tools\\C#\\app.exe".to_string();
        f.set(None, BindLine { flags: String::new(), mods: Mods::parse("SUPER").unwrap(), key: Key::parse("N").unwrap(), dispatcher: "exec".into(), arg: arg.clone() });
        f.set(None, BindLine { flags: String::new(), mods: Mods::parse("SUPER").unwrap(), key: Key::parse("M").unwrap(), dispatcher: "exec".into(), arg: "a\nbind = SUPER, Q, exit,".into() });
        let back = BindsFile::parse(&f.render());
        assert_eq!(back.binds.len(), 2, "{}", f.render());
        assert_eq!(back.binds[0].arg, arg);
        assert!(!back.binds.iter().any(|b| b.dispatcher == "exit"), "a line break injected a bind");
    }
}
