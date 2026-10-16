//! Talking to the running hyprwin daemon (same named pipe as hyprwinctl).

use std::path::PathBuf;

pub fn request(req: &str) -> Result<String, String> {
    hyprwin_ipc::request(req).map_err(|_| "hyprwin isn't running".to_string())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Running,
    Paused,
    NotRunning,
}

pub fn status() -> Status {
    match request("status").ok().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()) {
        Some(v) if v["paused"].as_bool() == Some(true) => Status::Paused,
        Some(_) => Status::Running,
        None => Status::NotRunning,
    }
}

pub fn reload() {
    let _ = request("reload");
}

/// hyprwin.exe next to this program.
pub fn daemon_exe() -> Option<PathBuf> {
    let p = std::env::current_exe().ok()?.with_file_name("hyprwin.exe");
    p.exists().then_some(p)
}

pub fn start_daemon() -> Result<(), String> {
    let exe = daemon_exe().ok_or("hyprwin.exe not found next to the settings app")?;
    std::process::Command::new(exe).spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[derive(Clone, Debug)]
pub struct BindRow {
    pub keys: String,
    pub action: String,
    pub description: String,
}

pub fn binds() -> Vec<BindRow> {
    let Ok(text) = request("j/binds") else { return Vec::new() };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else { return Vec::new() };
    v.as_array()
        .into_iter()
        .flatten()
        .map(|b| {
            let mods = b["mods"].as_str().unwrap_or("").replace('_', " + ");
            let key = b["key"].as_str().unwrap_or("").trim_start_matches("Vk(").trim_end_matches(')').to_string();
            let key = pretty_key(&key);
            BindRow {
                keys: if mods.is_empty() { key } else { format!("{mods} + {key}") },
                action: b["action"].as_str().map(str::to_string).unwrap_or_else(|| pretty_action(b["dispatcher"].as_str().unwrap_or(""))),
                description: b["description"].as_str().unwrap_or("").to_string(),
            }
        })
        .collect()
}

fn pretty_key(k: &str) -> String {
    if let Ok(vk) = k.parse::<u16>() {
        return match vk {
            0x30..=0x39 | 0x41..=0x5A => (vk as u8 as char).to_string(),
            0x70..=0x87 => format!("F{}", vk - 0x6F),
            0x0D => "Enter".into(),
            0x1B => "Esc".into(),
            0x20 => "Space".into(),
            0x09 => "Tab".into(),
            0x25 => "←".into(),
            0x26 => "↑".into(),
            0x27 => "→".into(),
            0x28 => "↓".into(),
            _ => format!("key {vk:#x}"),
        };
    }
    match k {
        "MouseLeft" => "Left mouse drag".into(),
        "MouseRight" => "Right mouse drag".into(),
        "WheelUp" => "Scroll up".into(),
        "WheelDown" => "Scroll down".into(),
        other => other.into(),
    }
}

fn pretty_action(d: &str) -> String {
    let lower = d.to_lowercase();
    let simple = [
        ("killactive", "Close window"),
        ("exit", "Quit hyprwin"),
        ("togglefloating", "Float / tile window"),
        ("pseudo", "Pseudotile"),
        ("mousemove", "Move window"),
        ("mouseresize", "Resize window"),
        ("togglepause", "Pause / resume tiling"),
        ("pin", "Pin window"),
    ];
    for (k, v) in simple {
        if lower.starts_with(k) {
            return v.into();
        }
    }
    d.replace("WsSel::", "").replace("Some(", "").replace("Id(", "").replace(")", "").replace("(", " ")
}
