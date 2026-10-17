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
