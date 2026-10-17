//! Hyprland key names (xkb keysyms) → Windows virtual-key codes.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Mods(pub u8);

impl Mods {
    pub const NONE: Mods = Mods(0);
    pub const SUPER: Mods = Mods(1);
    pub const SHIFT: Mods = Mods(2);
    pub const CTRL: Mods = Mods(4);
    pub const ALT: Mods = Mods(8);

    pub fn contains(self, o: Mods) -> bool {
        self.0 & o.0 == o.0
    }

    /// `SUPER_SHIFT`, `SUPER SHIFT`, `CTRL+ALT`, `MOD4`, ``.
    pub fn parse(s: &str) -> Result<Mods, String> {
        let mut m = 0u8;
        for tok in s.split(|c: char| c == '_' || c == ' ' || c == '+' || c == '&').filter(|t| !t.is_empty()) {
            m |= match tok.to_ascii_uppercase().as_str() {
                "SUPER" | "WIN" | "LOGO" | "MOD4" | "META" => 1,
                "SHIFT" => 2,
                "CTRL" | "CONTROL" => 4,
                "ALT" | "MOD1" => 8,
                "MOD2" | "MOD3" | "MOD5" | "CAPS" => 0,
                other => return Err(format!("unknown modifier '{other}'")),
            };
        }
        Ok(Mods(m))
    }
}

impl std::fmt::Display for Mods {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut parts = Vec::new();
        for (bit, name) in [(1, "SUPER"), (2, "SHIFT"), (4, "CTRL"), (8, "ALT")] {
            if self.0 & bit != 0 {
                parts.push(name);
            }
        }
        write!(f, "{}", parts.join("_"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    /// Windows virtual-key code.
    Vk(u16),
    /// Raw scancode from `code:NN` (xkb keycode − 8); mapped to a VK at runtime.
    Scan(u16),
    MouseLeft,
    MouseRight,
    MouseMiddle,
    MouseX1,
    MouseX2,
    WheelUp,
    WheelDown,
}

impl Key {
    /// The key as written in hyprland.conf (round-trips through [`parse`](Self::parse)).
    pub fn name(self) -> String {
        match self {
            Key::MouseLeft => "mouse:272".into(),
            Key::MouseRight => "mouse:273".into(),
            Key::MouseMiddle => "mouse:274".into(),
            Key::MouseX1 => "mouse:275".into(),
            Key::MouseX2 => "mouse:276".into(),
            Key::WheelUp => "mouse_up".into(),
            Key::WheelDown => "mouse_down".into(),
            Key::Scan(n) => format!("code:{}", n + 8),
            Key::Vk(vk) => match vk {
                0x30..=0x39 | 0x41..=0x5A => (vk as u8 as char).to_string(),
                0x70..=0x87 => format!("F{}", vk - 0x6F),
                0x60..=0x69 => format!("KP_{}", vk - 0x60),
                _ => NAMED.iter().find(|(_, v)| *v == vk).map(|(n, _)| n.to_string()).unwrap_or_else(|| format!("code:{vk}")),
            },
        }
    }

    /// Short, friendly label for the Settings window.
    pub fn label(self) -> String {
        match self {
            Key::MouseLeft => "Left mouse".into(),
            Key::MouseRight => "Right mouse".into(),
            Key::MouseMiddle => "Middle mouse".into(),
            Key::MouseX1 => "Mouse back".into(),
            Key::MouseX2 => "Mouse forward".into(),
            Key::WheelUp => "Scroll up".into(),
            Key::WheelDown => "Scroll down".into(),
            Key::Vk(0x25) => "←".into(),
            Key::Vk(0x26) => "↑".into(),
            Key::Vk(0x27) => "→".into(),
            Key::Vk(0x28) => "↓".into(),
            Key::Vk(0x0D) => "Enter".into(),
            Key::Vk(0x20) => "Space".into(),
            Key::Vk(0x1B) => "Esc".into(),
            Key::Vk(0x09) => "Tab".into(),
            other => other.name(),
        }
    }

    pub fn is_mouse(self) -> bool {
        !matches!(self, Key::Vk(_) | Key::Scan(_))
    }

    pub fn parse(s: &str) -> Result<Key, String> {
        let t = s.trim();
        if let Some(code) = t.strip_prefix("code:") {
            let n: u16 = code.parse().map_err(|_| format!("bad keycode '{t}'"))?;
            return Ok(Key::Scan(n.saturating_sub(8)));
        }
        if let Some(b) = t.strip_prefix("mouse:") {
            return match b {
                "272" => Ok(Key::MouseLeft),
                "273" => Ok(Key::MouseRight),
                "274" => Ok(Key::MouseMiddle),
                "275" => Ok(Key::MouseX1),
                "276" => Ok(Key::MouseX2),
                _ => Err(format!("unsupported mouse button '{t}'")),
            };
        }
        match t {
            "mouse_up" => return Ok(Key::WheelUp),
            "mouse_down" => return Ok(Key::WheelDown),
            _ => {}
        }
        if t.chars().count() == 1 {
            let c = t.chars().next().unwrap().to_ascii_uppercase();
            if c.is_ascii_alphanumeric() {
                return Ok(Key::Vk(c as u16));
            }
        }
        let lower = t.to_ascii_lowercase();
        if let Some(n) = lower.strip_prefix('f').and_then(|n| n.parse::<u16>().ok()) {
            if (1..=24).contains(&n) {
                return Ok(Key::Vk(0x6F + n));
            }
        }
        if let Some(n) = lower.strip_prefix("kp_").and_then(|n| n.parse::<u16>().ok()) {
            if n <= 9 {
                return Ok(Key::Vk(0x60 + n));
            }
        }
        let vk: u16 = match lower.as_str() {
            "return" | "enter" | "kp_enter" => 0x0D,
            "tab" => 0x09,
            "escape" | "esc" => 0x1B,
            "space" => 0x20,
            "backspace" => 0x08,
            "delete" => 0x2E,
            "insert" => 0x2D,
            "home" => 0x24,
            "end" => 0x23,
            "prior" | "page_up" | "pageup" => 0x21,
            "next" | "page_down" | "pagedown" => 0x22,
            "left" => 0x25,
            "up" => 0x26,
            "right" => 0x27,
            "down" => 0x28,
            "print" => 0x2C,
            "pause" => 0x13,
            "scroll_lock" => 0x91,
            "caps_lock" => 0x14,
            "menu" => 0x5D,
            "minus" => 0xBD,
            "equal" | "plus" => 0xBB,
            "comma" => 0xBC,
            "period" => 0xBE,
            "slash" => 0xBF,
            "semicolon" => 0xBA,
            "apostrophe" => 0xDE,
            "grave" => 0xC0,
            "bracketleft" => 0xDB,
            "bracketright" => 0xDD,
            "backslash" => 0xDC,
            "kp_add" => 0x6B,
            "kp_subtract" => 0x6D,
            "kp_multiply" => 0x6A,
            "kp_divide" => 0x6F,
            "kp_decimal" => 0x6E,
            "super_l" => 0x5B,
            "super_r" => 0x5C,
            "shift_l" => 0xA0,
            "shift_r" => 0xA1,
            "control_l" => 0xA2,
            "control_r" => 0xA3,
            "alt_l" => 0xA4,
            "alt_r" => 0xA5,
            "xf86audioraisevolume" => 0xAF,
            "xf86audiolowervolume" => 0xAE,
            "xf86audiomute" => 0xAD,
            "xf86audioplay" | "xf86audiopause" => 0xB3,
            "xf86audionext" => 0xB0,
            "xf86audioprev" => 0xB1,
            "xf86audiostop" => 0xB2,
            "xf86mail" => 0xB4,
            "xf86calculator" => 0xB7,
            "xf86search" => 0xAA,
            "xf86homepage" => 0xAC,
            _ => return Err(format!("unknown or unsupported key '{t}'")),
        };
        Ok(Key::Vk(vk))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mods() {
        assert_eq!(Mods::parse("SUPER_SHIFT").unwrap(), Mods(3));
        assert_eq!(Mods::parse("SUPER SHIFT CTRL").unwrap(), Mods(7));
        assert_eq!(Mods::parse("").unwrap(), Mods::NONE);
        assert!(Mods::parse("HYPER").is_err());
    }

    #[test]
    fn keys() {
        assert_eq!(Key::parse("Q").unwrap(), Key::Vk(0x51));
        assert_eq!(Key::parse("q").unwrap(), Key::Vk(0x51));
        assert_eq!(Key::parse("1").unwrap(), Key::Vk(0x31));
        assert_eq!(Key::parse("Return").unwrap(), Key::Vk(0x0D));
        assert_eq!(Key::parse("left").unwrap(), Key::Vk(0x25));
        assert_eq!(Key::parse("F12").unwrap(), Key::Vk(0x7B));
        assert_eq!(Key::parse("mouse:272").unwrap(), Key::MouseLeft);
        assert_eq!(Key::parse("mouse_down").unwrap(), Key::WheelDown);
        assert_eq!(Key::parse("code:10").unwrap(), Key::Scan(2));
        assert_eq!(Key::parse("XF86AudioRaiseVolume").unwrap(), Key::Vk(0xAF));
        assert!(Key::parse("XF86MonBrightnessUp").is_err());
    }
}

/// Canonical hyprland.conf names of named keys (used to write keys back).
const NAMED: &[(&str, u16)] = &[
    ("Return", 0x0D),
    ("Tab", 0x09),
    ("Escape", 0x1B),
    ("space", 0x20),
    ("BackSpace", 0x08),
    ("Delete", 0x2E),
    ("Insert", 0x2D),
    ("Home", 0x24),
    ("End", 0x23),
    ("Prior", 0x21),
    ("Next", 0x22),
    ("left", 0x25),
    ("up", 0x26),
    ("right", 0x27),
    ("down", 0x28),
    ("Print", 0x2C),
    ("Pause", 0x13),
    ("Scroll_Lock", 0x91),
    ("Caps_Lock", 0x14),
    ("Menu", 0x5D),
    ("minus", 0xBD),
    ("equal", 0xBB),
    ("comma", 0xBC),
    ("period", 0xBE),
    ("slash", 0xBF),
    ("semicolon", 0xBA),
    ("apostrophe", 0xDE),
    ("grave", 0xC0),
    ("bracketleft", 0xDB),
    ("bracketright", 0xDD),
    ("backslash", 0xDC),
    ("KP_Add", 0x6B),
    ("KP_Subtract", 0x6D),
    ("KP_Multiply", 0x6A),
    ("KP_Divide", 0x6F),
    ("KP_Decimal", 0x6E),
];

#[cfg(test)]
mod name_tests {
    use super::*;

    #[test]
    fn every_named_key_round_trips() {
        for (name, vk) in NAMED {
            assert_eq!(Key::parse(name).unwrap(), Key::Vk(*vk), "{name}");
            assert_eq!(Key::Vk(*vk).name(), *name);
        }
        for k in ["Q", "7", "F5", "F12", "KP_3", "mouse:272", "mouse_down", "code:10"] {
            assert_eq!(Key::parse(&Key::parse(k).unwrap().name()).unwrap(), Key::parse(k).unwrap(), "{k}");
        }
    }
}
