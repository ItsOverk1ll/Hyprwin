//! Window rules in all hyprlang dialects.
//!
//! - `windowrulev2 = float, class:^(foo)$, title:bar`             (≤ 0.47)
//! - `windowrule = float, ^(foo)$` / `windowrule = float, title:x` (v1, ≤ 0.47)
//! - `windowrule = match:class foo, float on`                     (0.48 – 0.54)
//! - `windowrule { match:class = foo; float = on }`               (0.48 – 0.54)
//!
//! Hyprland `class` maps to the **process name without `.exe`** (lowercase);
//! `match:winclass` (extension) matches the Win32 window class; `match:exe`
//! (extension) matches the full executable path.

use regex::Regex;

use crate::dispatch::Delta;
use crate::model::Color;

#[derive(Clone, Debug, Default)]
pub struct WindowProps {
    pub class: String,
    pub title: String,
    pub initial_class: String,
    pub initial_title: String,
    pub winclass: String,
    pub exe: String,
    pub floating: bool,
    pub fullscreen: bool,
    pub pinned: bool,
    pub focused: bool,
    pub workspace: String,
}

#[derive(Clone, Debug)]
enum Field {
    Class,
    Title,
    InitialClass,
    InitialTitle,
    WinClass,
    Exe,
    Float,
    Fullscreen,
    Pin,
    Focus,
    Workspace,
    XWayland,
}

#[derive(Clone, Debug)]
struct Matcher {
    field: Field,
    re: Option<Regex>,
    flag: Option<bool>,
    negate: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RuleEffect {
    Float,
    Tile,
    Fullscreen,
    Maximize,
    Pin,
    Pseudo,
    Center,
    Size(Delta, Delta),
    Move(Delta, Delta),
    Workspace { ws: String, silent: bool },
    Monitor(String),
    NoInitialFocus,
    NoFocus,
    Opacity(f64),
    BorderColor(Color),
    Rounding(u32),
    NoAnim,
    Animation(String),
    /// hyprwin extension: never manage this window.
    Unmanaged,
}

#[derive(Clone, Debug)]
pub struct WindowRule {
    matchers: Vec<Matcher>,
    pub effects: Vec<RuleEffect>,
    pub source: String,
}

impl WindowRule {
    pub fn matches(&self, p: &WindowProps) -> bool {
        !self.matchers.is_empty()
            && self.matchers.iter().all(|m| {
                let hit = match (&m.field, &m.re, m.flag) {
                    (Field::Class, Some(r), _) => r.is_match(&p.class),
                    (Field::Title, Some(r), _) => r.is_match(&p.title),
                    (Field::InitialClass, Some(r), _) => r.is_match(&p.initial_class),
                    (Field::InitialTitle, Some(r), _) => r.is_match(&p.initial_title),
                    (Field::WinClass, Some(r), _) => r.is_match(&p.winclass),
                    (Field::Exe, Some(r), _) => r.is_match(&p.exe),
                    (Field::Workspace, Some(r), _) => r.is_match(&p.workspace),
                    (Field::Float, _, Some(f)) => p.floating == f,
                    (Field::Fullscreen, _, Some(f)) => p.fullscreen == f,
                    (Field::Pin, _, Some(f)) => p.pinned == f,
                    (Field::Focus, _, Some(f)) => p.focused == f,
                    (Field::XWayland, _, Some(f)) => !f, // nothing is XWayland on Windows
                    _ => false,
                };
                hit != m.negate
            })
    }
}

fn bool_of(s: &str) -> Option<bool> {
    match s.trim().to_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn field_of(name: &str) -> Option<Field> {
    Some(match name.to_lowercase().as_str() {
        "class" => Field::Class,
        "title" => Field::Title,
        "initialclass" | "initial_class" => Field::InitialClass,
        "initialtitle" | "initial_title" => Field::InitialTitle,
        "winclass" => Field::WinClass,
        "exe" => Field::Exe,
        "floating" | "float" => Field::Float,
        "fullscreen" => Field::Fullscreen,
        "pinned" | "pin" => Field::Pin,
        "focus" => Field::Focus,
        "workspace" | "onworkspace" => Field::Workspace,
        "xwayland" => Field::XWayland,
        _ => return None,
    })
}

fn matcher(field_name: &str, value: &str) -> Result<Matcher, String> {
    let field = field_of(field_name).ok_or_else(|| format!("unsupported match field '{field_name}'"))?;
    let value = value.trim();
    let (negate, value) = match value.strip_prefix("negative:") {
        Some(v) => (true, v),
        None => (false, value),
    };
    match field {
        Field::Float | Field::Fullscreen | Field::Pin | Field::Focus | Field::XWayland => {
            Ok(Matcher { field, re: None, flag: Some(bool_of(value).ok_or_else(|| format!("expected bool, got '{value}'"))?), negate })
        }
        _ => {
            // Hyprland regexes are full-match by convention (`^(…)$`); we match
            // case-insensitively because Windows process names have no canonical case.
            let re = Regex::new(&format!("(?i){value}")).map_err(|e| format!("bad regex '{value}': {e}"))?;
            Ok(Matcher { field, re: Some(re), flag: None, negate })
        }
    }
}

fn parse_xy(a: &str) -> Result<(Delta, Delta), String> {
    let t: Vec<&str> = a.split_whitespace().collect();
    if t.len() != 2 {
        return Err(format!("expected 'X Y', got '{a}'"));
    }
    let d = |s: &str| -> Result<Delta, String> {
        // Hyprland allows expressions like `monitor_w*0.5`; support the % form and plain numbers.
        let s = s.trim();
        let (n, p) = match s.strip_suffix('%') {
            Some(n) => (n, true),
            None => (s, false),
        };
        n.parse::<f64>().map(|value| Delta { value, percent: p }).map_err(|_| format!("unsupported size/position expression '{s}'"))
    };
    Ok((d(t[0])?, d(t[1])?))
}

/// Parses one effect: name + optional args (`size 800 600`, `float on`, `workspace 3 silent`).
pub fn effect(name: &str, args: &str) -> Result<Option<RuleEffect>, String> {
    let name = name.trim().to_lowercase();
    let args = args.trim();
    // New-style toggles: `float on` / `float off`.
    if let Some(b) = bool_of(args) {
        if !b {
            return Ok(None);
        }
    }
    Ok(Some(match name.as_str() {
        "float" => RuleEffect::Float,
        "tile" => RuleEffect::Tile,
        "fullscreen" => RuleEffect::Fullscreen,
        "maximize" => RuleEffect::Maximize,
        "pin" => RuleEffect::Pin,
        "pseudo" => RuleEffect::Pseudo,
        "center" => RuleEffect::Center,
        "size" => {
            let (x, y) = parse_xy(args)?;
            RuleEffect::Size(x, y)
        }
        "move" => {
            let (x, y) = parse_xy(args)?;
            RuleEffect::Move(x, y)
        }
        "workspace" => {
            let silent = args.split_whitespace().any(|t| t == "silent");
            let ws = args.split_whitespace().find(|t| *t != "silent").unwrap_or("").to_string();
            RuleEffect::Workspace { ws, silent }
        }
        "monitor" => RuleEffect::Monitor(args.to_string()),
        "noinitialfocus" | "no_initial_focus" => RuleEffect::NoInitialFocus,
        "nofocus" | "no_focus" => RuleEffect::NoFocus,
        "opacity" => RuleEffect::Opacity(args.split_whitespace().next().and_then(|v| v.parse().ok()).unwrap_or(1.0)),
        "bordercolor" | "border_color" => RuleEffect::BorderColor(Color::parse(args).ok_or_else(|| format!("bad colour '{args}'"))?),
        "rounding" => RuleEffect::Rounding(args.parse().unwrap_or(0)),
        "noanim" | "no_anim" => RuleEffect::NoAnim,
        "animation" => RuleEffect::Animation(args.to_string()),
        "unmanaged" | "ignore" => RuleEffect::Unmanaged,
        // Wayland/compositor-only effects: accept silently-but-warned by caller.
        _ => return Err(format!("window rule effect '{name}' is not supported on Windows")),
    }))
}

fn split_name_args(s: &str) -> (&str, &str) {
    let s = s.trim();
    match s.split_once(char::is_whitespace) {
        Some((a, b)) => (a, b.trim()),
        None => (s, ""),
    }
}

/// Splits on commas that are not inside parentheses (regexes like `^(a|b)$` are fine,
/// but `(a,b)` must not be split).
fn split_top_level(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    for c in s.chars() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => {
                out.push(std::mem::take(&mut cur));
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    out.push(cur);
    out.into_iter().map(|x| x.trim().to_string()).collect()
}

pub struct ParsedRule {
    pub rule: Option<WindowRule>,
    pub warnings: Vec<String>,
}

/// `windowrulev2 = EFFECT, field:regex, …`
pub fn parse_v2(value: &str, source: &str) -> ParsedRule {
    let parts = split_top_level(value);
    let mut warnings = Vec::new();
    let (name, args) = split_name_args(&parts[0]);
    let eff = match effect(name, args) {
        Ok(e) => e,
        Err(e) => return ParsedRule { rule: None, warnings: vec![format!("{source}: {e}")] },
    };
    let mut matchers = Vec::new();
    for p in &parts[1..] {
        let Some((f, v)) = p.split_once(':') else {
            warnings.push(format!("{source}: bad matcher '{p}'"));
            continue;
        };
        match matcher(f, v) {
            Ok(m) => matchers.push(m),
            Err(e) => return ParsedRule { rule: None, warnings: vec![format!("{source}: {e}")] },
        }
    }
    ParsedRule { rule: Some(WindowRule { matchers, effects: eff.into_iter().collect(), source: source.into() }), warnings }
}

/// `windowrule = …`: detects the 0.48+ `match:` syntax vs the legacy v1 syntax.
pub fn parse_windowrule(value: &str, source: &str) -> ParsedRule {
    let parts = split_top_level(value);
    if !parts.iter().any(|p| p.starts_with("match:")) {
        // v1: `windowrule = EFFECT, REGEX` where REGEX matches class, or `title:REGEX`.
        if parts.len() < 2 {
            return ParsedRule { rule: None, warnings: vec![format!("{source}: windowrule needs an effect and a window")] };
        }
        let win = parts[1..].join(",");
        let v2 = if win.contains(':') && !win.starts_with('^') { format!("{}, {}", parts[0], win) } else { format!("{}, class:{}", parts[0], win) };
        return parse_v2(&v2, source);
    }
    let mut kv = Vec::new();
    for p in parts {
        if let Some(m) = p.strip_prefix("match:") {
            let (f, v) = split_name_args(m);
            kv.push((format!("match:{f}"), v.to_string()));
        } else {
            let (n, a) = split_name_args(&p);
            kv.push((n.to_string(), a.to_string()));
        }
    }
    parse_block(&kv, source)
}

/// `windowrule { name = …; match:class = …; float = on }`
pub fn parse_block(kv: &[(String, String)], source: &str) -> ParsedRule {
    let mut warnings = Vec::new();
    let mut matchers = Vec::new();
    let mut effects = Vec::new();
    for (k, v) in kv {
        if k == "name" || k == "enable" {
            continue;
        }
        if let Some(f) = k.strip_prefix("match:") {
            match matcher(f, v) {
                Ok(m) => matchers.push(m),
                Err(e) => return ParsedRule { rule: None, warnings: vec![format!("{source}: {e}")] },
            }
        } else {
            match effect(k, v) {
                Ok(Some(e)) => effects.push(e),
                Ok(None) => {}
                Err(e) => warnings.push(format!("{source}: {e}")),
            }
        }
    }
    if matchers.is_empty() {
        return ParsedRule { rule: None, warnings: vec![format!("{source}: window rule has no match: props")] };
    }
    ParsedRule { rule: Some(WindowRule { matchers, effects, source: source.into() }), warnings }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(class: &str, title: &str) -> WindowProps {
        WindowProps { class: class.into(), title: title.into(), ..Default::default() }
    }

    #[test]
    fn v2_syntax() {
        let r = parse_v2("float, class:^(pavucontrol|calc)$, title:negative:^Settings$", "t").rule.unwrap();
        assert!(r.matches(&props("calc", "x")));
        assert!(!r.matches(&props("calc", "Settings")));
        assert!(!r.matches(&props("notepad", "x")));
        assert_eq!(r.effects, vec![RuleEffect::Float]);
    }

    #[test]
    fn v2_with_args() {
        let r = parse_v2("size 800 600, class:^(steam)$", "t").rule.unwrap();
        assert_eq!(r.effects, vec![RuleEffect::Size(Delta { value: 800.0, percent: false }, Delta { value: 600.0, percent: false })]);
        let r = parse_v2("workspace 3 silent, class:discord", "t").rule.unwrap();
        assert_eq!(r.effects, vec![RuleEffect::Workspace { ws: "3".into(), silent: true }]);
    }

    #[test]
    fn match_syntax_anonymous() {
        let r = parse_windowrule("match:class ^(firefox)$, match:title .*Picture-in-Picture.*, float on, pin on", "t").rule.unwrap();
        assert!(r.matches(&props("firefox", "Picture-in-Picture")));
        assert_eq!(r.effects, vec![RuleEffect::Float, RuleEffect::Pin]);
    }

    #[test]
    fn legacy_v1() {
        let r = parse_windowrule("float, ^(Calculator)$", "t").rule.unwrap();
        assert!(r.matches(&props("calculator", "")), "case-insensitive class");
        let r = parse_windowrule("float, title:^(Open File)$", "t").rule.unwrap();
        assert!(r.matches(&props("x", "Open File")));
    }

    #[test]
    fn block_syntax_and_warnings() {
        let kv = vec![
            ("name".to_string(), "x".to_string()),
            ("match:class".to_string(), "code".to_string()),
            ("float".to_string(), "on".to_string()),
            ("blur".to_string(), "off".to_string()),
            ("idle_inhibit".to_string(), "fullscreen".to_string()),
        ];
        let p = parse_block(&kv, "t");
        let r = p.rule.unwrap();
        assert_eq!(r.effects, vec![RuleEffect::Float]);
        assert_eq!(p.warnings.len(), 1, "{:?}", p.warnings); // idle_inhibit unsupported; blur off is a no-op
    }

    #[test]
    fn float_off_is_noop() {
        let r = parse_windowrule("match:class x, float off", "t").rule.unwrap();
        assert!(r.effects.is_empty());
    }
}
