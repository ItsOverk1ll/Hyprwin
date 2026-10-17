//! The config is the user's own text and is reloaded on every save, on the
//! daemon's main thread: no input may make parsing panic. These tests throw
//! mutated configs and junk at every public parser.

use hyprwin_config::{Color, Config, Gradient, Key, Mods, dispatch, rules, themes};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// Fragments that tend to reach edge cases in a hyprlang parser.
const PIECES: &[&str] = &[
    "=", ",", ":", "{", "}", "#", "##", "$", "$var", "\\", "\"", "'", " ", "\t", "\n", "\r\n", "-", "+", "-1", "0", "0x", "0xff",
    "rgba(", "rgb(", ")", "ff00ffaa", "deg", "45deg", "999999999999999999999", "1e309", "NaN", "-0", "%", "50%", "exact",
    "SUPER", "SHIFT", "CTRL", "ALT", "MOD5", "mouse:272", "mouse:", "code:", "code:99999", "bind", "bindm", "binde", "bindrl",
    "unbind", "submap", "reset", "exec", "workspace", "special:", "name:", "e+1", "m-1", "r~1", "windowrule", "windowrulev2",
    "match:", "class:", "title:^(", "regex(", "[", "]", "(", "float", "size", "move", "100%-w", "animation", "bezier",
    "source", "~/", "../../", "monitor", "general", "decoration", "dwindle", "master", "gaps_in", "col.active_border",
    "\u{0}", "é", "😀", "\u{202e}", "workspace_swipe", "env", "layoutmsg", "resizeactive", "movewindow", "l", "u",
];

fn junk(rng: &mut Rng, n: usize) -> String {
    (0..n).map(|_| PIECES[rng.below(PIECES.len())]).collect()
}

fn mutate(rng: &mut Rng, text: &str) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    for _ in 0..1 + rng.below(6) {
        if lines.is_empty() {
            lines.push(String::new());
        }
        let i = rng.below(lines.len());
        match rng.below(6) {
            0 => {
                // Truncate mid-line at a char boundary.
                let cut: Vec<char> = lines[i].chars().collect();
                let k = rng.below(cut.len() + 1);
                lines[i] = cut[..k].iter().collect();
            }
            1 => {
                let n = 1 + rng.below(4);
                lines[i].push_str(&junk(rng, n));
            }
            2 => {
                let n = 1 + rng.below(6);
                let j = junk(rng, n);
                lines.insert(i, j);
            }
            3 => {
                lines.remove(i);
            }
            4 => {
                let pos = lines[i].char_indices().map(|(k, _)| k).nth(rng.below(lines[i].chars().count() + 1)).unwrap_or(lines[i].len());
                let j = junk(rng, 1);
                lines[i].insert_str(pos, &j);
            }
            _ => {
                let j = rng.below(lines.len());
                lines.swap(i, j);
            }
        }
    }
    lines.join("\n")
}

#[test]
fn mutated_configs_never_panic() {
    let base = hyprwin_config::DEFAULT_CONFIG;
    let mut rng = Rng(0x5eed_cafe);
    for round in 0..4000 {
        let text = mutate(&mut rng, base);
        let r = std::panic::catch_unwind(|| {
            let cfg = Config::load_str(&text);
            let _ = cfg.effective_binds();
            for name in ["windows", "windowsIn", "windowsOut", "windowsMove", "workspaces", "workspacesIn", "specialWorkspace", "fade", "border", "global"] {
                let _ = cfg.anim(name);
            }
        });
        if r.is_err() {
            panic!("round {round}: config parsing panicked on:\n-----\n{text}\n-----");
        }
    }
}

#[test]
fn junk_lines_never_panic() {
    let mut rng = Rng(0xfeed_beef);
    for round in 0..4000 {
        let lines = 1 + rng.below(8);
        let mut text = String::new();
        for _ in 0..lines {
            let n = 1 + rng.below(10);
            text += &junk(&mut rng, n);
            text.push('\n');
        }
        let r = std::panic::catch_unwind(|| Config::load_str(&text));
        if r.is_err() {
            panic!("round {round}: config parsing panicked on:\n-----\n{text:?}\n-----");
        }
    }
}

#[test]
fn value_parsers_never_panic() {
    let mut rng = Rng(0x0dd_ba11);
    let dispatchers = ["exec", "workspace", "movetoworkspace", "movetoworkspacesilent", "resizeactive", "moveactive", "movefocus", "movewindow", "fullscreen", "layoutmsg", "togglespecialworkspace", "focusmonitor", "moveworkspacetomonitor", "swapactiveworkspaces", "renameworkspace", "closewindow", "focuswindow", "splitratio", "cyclenext", "pass", "sendshortcut", "submap", "togglefloating", "pin", "resizewindowpixel", "movewindowpixel"];
    for round in 0..20000 {
        let n = rng.below(8);
        let s = junk(&mut rng, n);
        let d = dispatchers[rng.below(dispatchers.len())];
        let r = std::panic::catch_unwind(|| {
            let _ = Color::parse(&s);
            let _ = Gradient::parse(&s);
            let _ = Key::parse(&s);
            let _ = Mods::parse(&s);
            let _ = dispatch::parse(d, &s);
            let _ = dispatch::parse(&s, &s);
            let _ = rules::parse_v2(&s, "fuzz");
            let _ = rules::parse_windowrule(&s, "fuzz");
            let _ = rules::effect(&s, &s);
            let _ = hyprwin_config::model::normalize_monitor(&s);
            let _ = themes::active_theme(&s);
            let _ = themes::with_managed_block(&s, None);
            let _ = hyprwin_config::binds_file::BindsFile::parse(&s);
            let _ = hyprwin_config::settings::SettingsFile::parse(&s);
            let _ = hyprwin_config::binds_file::match_action(d, &s);
        });
        if r.is_err() {
            panic!("round {round}: a value parser panicked on dispatcher {d:?}, input {s:?}");
        }
    }
}
