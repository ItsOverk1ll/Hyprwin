//! Where hyprwin keeps its files. Shared by the daemon and the settings app.

use std::path::PathBuf;

/// `%APPDATA%\hyprwin` (themes, groups, settings.conf, default config).
pub fn app_dir() -> PathBuf {
    std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("hyprwin")
}

pub fn themes_dir() -> PathBuf {
    app_dir().join("themes")
}

pub fn groups_dir() -> PathBuf {
    app_dir().join("groups")
}

/// Values chosen in the Settings window. Sourced last, so they win.
pub fn settings_conf() -> PathBuf {
    app_dir().join("settings.conf")
}

/// Keybind changes made in the Settings window. Sourced after settings.conf.
pub fn binds_conf() -> PathBuf {
    app_dir().join("binds.conf")
}

/// Config locations in priority order (Hyprland's path first).
pub fn config_candidates() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(home) = std::env::var_os("USERPROFILE") {
        v.push(PathBuf::from(home).join(".config").join("hypr").join("hyprland.conf"));
    }
    v.push(app_dir().join("hyprland.conf"));
    v
}

/// The config file in use (first existing candidate, else the default location).
pub fn active_config() -> PathBuf {
    let c = config_candidates();
    c.iter().find(|p| p.exists()).cloned().unwrap_or_else(|| c.last().cloned().unwrap())
}

/// Turns a group/theme name into a safe file stem.
pub fn file_stem(name: &str) -> String {
    let s: String = name.trim().chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' { c } else { '_' }).collect();
    let s = s.trim().to_string();
    if s.is_empty() {
        return "untitled".into();
    }
    // Windows device names (CON, NUL, COM1…) are not usable as file names.
    const RESERVED: &[&str] = &["con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8", "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9"];
    if RESERVED.contains(&s.to_lowercase().as_str()) { format!("_{s}") } else { s }
}

#[cfg(test)]
mod tests {
    #[test]
    fn file_stems_are_safe() {
        assert_eq!(super::file_stem("Coding: VS Code + Claude"), "Coding_ VS Code _ Claude");
        assert_eq!(super::file_stem("../../evil"), "______evil");
        assert_eq!(super::file_stem("  "), "untitled");
        assert_eq!(super::file_stem("NUL"), "_NUL");
        assert_eq!(super::file_stem("com1"), "_com1");
        assert_eq!(super::file_stem("console"), "console");
    }
}
