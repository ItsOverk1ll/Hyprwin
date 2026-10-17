//! Themes: shareable hyprlang snippets.
//!
//! A theme is any `.conf` file in `%APPDATA%\hyprwin\themes`, written in
//! normal hyprland.conf syntax (colours, gaps, borders, rounding, animations),
//! with optional header comments:
//!
//! ```text
//! # name: Nord
//! # author: someone
//! # description: Cool blue-grey borders.
//! general {
//!     col.active_border = rgba(88c0d0ff)
//! }
//! ```
//!
//! The active theme is chosen by a `source = …` line in a block hyprwin
//! manages at the end of the user's config (see [`managed_block`]).

use std::path::{Path, PathBuf};

use crate::paths;

#[derive(Clone, Debug, PartialEq)]
pub struct ThemeInfo {
    pub name: String,
    pub author: String,
    pub description: String,
    pub path: PathBuf,
    /// Parsed colours for previews (0xAARRGGBB).
    pub active: Option<u32>,
    /// All stops of `col.active_border` (one entry when solid).
    pub active_gradient: Option<crate::Gradient>,
    pub inactive: Option<u32>,
    pub gaps_in: Option<String>,
    pub gaps_out: Option<String>,
    pub rounding: Option<String>,
}

/// Built-in themes written to the themes folder on first use.
pub const BUILTIN: &[(&str, &str)] = &[
    (
        "hyprland-default",
        "# name: Hyprland Default\n# author: hyprwin\n# description: Hyprland's stock cyan look.\ngeneral {\n    gaps_in = 5\n    gaps_out = 20\n    border_size = 2\n    col.active_border = rgba(33ccffee)\n    col.inactive_border = rgba(595959aa)\n}\ndecoration {\n    rounding = 10\n}\n",
    ),
    (
        "hyprland-gradient",
        "# name: Hyprland Gradient
# author: hyprwin
# description: Hyprland's signature cyan-to-green gradient border, 3 px.
general {
    gaps_in = 5
    gaps_out = 20
    border_size = 3
    col.active_border = rgba(33ccffee) rgba(00ff99ee) 45deg
    col.inactive_border = rgba(595959aa)
}
decoration {
    rounding = 10
}
",
    ),
    (
        "catppuccin-mocha",
        "# name: Catppuccin Mocha\n# author: hyprwin\n# description: Soft mauve on dark, rounded and roomy.\ngeneral {\n    gaps_in = 6\n    gaps_out = 16\n    border_size = 2\n    col.active_border = rgba(cba6f7ff)\n    col.inactive_border = rgba(45475aff)\n}\ndecoration {\n    rounding = 10\n}\n",
    ),
    (
        "nord",
        "# name: Nord\n# author: hyprwin\n# description: Cool frost-blue borders, medium gaps.\ngeneral {\n    gaps_in = 5\n    gaps_out = 14\n    border_size = 2\n    col.active_border = rgba(88c0d0ff)\n    col.inactive_border = rgba(3b4252ff)\n}\ndecoration {\n    rounding = 8\n}\n",
    ),
    (
        "gruvbox",
        "# name: Gruvbox\n# author: hyprwin\n# description: Warm retro orange, small corners.\ngeneral {\n    gaps_in = 4\n    gaps_out = 12\n    border_size = 2\n    col.active_border = rgba(fe8019ff)\n    col.inactive_border = rgba(3c3836ff)\n}\ndecoration {\n    rounding = 4\n}\n",
    ),
    (
        "tokyo-night",
        "# name: Tokyo Night\n# author: hyprwin\n# description: Neon blue and purple on midnight.\ngeneral {\n    gaps_in = 5\n    gaps_out = 18\n    border_size = 2\n    col.active_border = rgba(7aa2f7ff)\n    col.inactive_border = rgba(414868ff)\n}\ndecoration {\n    rounding = 10\n}\n",
    ),
    (
        "minimal",
        "# name: Minimal\n# author: hyprwin\n# description: No gaps, square corners, quiet grey borders. Maximum space.\ngeneral {\n    gaps_in = 0\n    gaps_out = 0\n    border_size = 1\n    col.active_border = rgba(aaaaaaff)\n    col.inactive_border = rgba(333333ff)\n}\ndecoration {\n    rounding = 0\n}\n",
    ),
];

/// Writes built-in themes that are missing (never overwrites user edits).
pub fn install_builtins() {
    let dir = paths::themes_dir();
    let _ = std::fs::create_dir_all(&dir);
    for (stem, text) in BUILTIN {
        let p = dir.join(format!("{stem}.conf"));
        if !p.exists() {
            let _ = std::fs::write(p, text);
        }
    }
}

pub fn read_info(path: &Path) -> Option<ThemeInfo> {
    let text = std::fs::read_to_string(path).ok()?;
    let header = |k: &str| {
        text.lines()
            .filter_map(|l| l.trim().strip_prefix('#'))
            .filter_map(|l| l.trim().strip_prefix(k))
            .filter_map(|l| l.trim_start().strip_prefix(':'))
            .map(|v| v.trim().to_string())
            .next()
    };
    let parsed = crate::parse::parse_str(&text, &path.display().to_string());
    let get = |key: &str| parsed.entries.iter().rev().find(|e| e.key == key).map(|e| e.value.clone());
    let stem = path.file_stem()?.to_string_lossy().to_string();
    Some(ThemeInfo {
        name: header("name").unwrap_or_else(|| stem.clone()),
        author: header("author").unwrap_or_default(),
        description: header("description").unwrap_or_default(),
        path: path.to_path_buf(),
        active: get("general:col.active_border").and_then(|v| crate::Color::parse(&v)).map(|c| c.0),
        active_gradient: get("general:col.active_border").and_then(|v| crate::Gradient::parse(&v)),
        inactive: get("general:col.inactive_border").and_then(|v| crate::Color::parse(&v)).map(|c| c.0),
        gaps_in: get("general:gaps_in"),
        gaps_out: get("general:gaps_out"),
        rounding: get("decoration:rounding"),
    })
}

pub fn list() -> Vec<ThemeInfo> {
    let mut v: Vec<ThemeInfo> = std::fs::read_dir(paths::themes_dir())
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "conf")).filter_map(|p| read_info(&p)).collect())
        .unwrap_or_default();
    v.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    v
}

// ------------------------------------------------------------------ managed block

const BLOCK_START: &str = "# ---- hyprwin: theme and Settings window (managed, keep at the end) ----";
const BLOCK_END: &str = "# ---- end hyprwin managed block ----";

/// The block appended to the user's config: the active theme, then the
/// Settings window's values (so settings win over the theme, and both win
/// over everything above).
pub fn managed_block(theme: Option<&Path>) -> String {
    let theme_line = match theme {
        Some(p) => format!("source = {}", p.display()),
        None => "# (no theme selected)".to_string(),
    };
    format!("{BLOCK_START}\n{theme_line}\nsource = {}\nsource = {}\n{BLOCK_END}\n", paths::settings_conf().display(), paths::binds_conf().display())
}

/// Path of the theme currently selected in the managed block, if any.
pub fn active_theme(config_text: &str) -> Option<PathBuf> {
    let start = config_text.find(BLOCK_START)?;
    config_text[start..]
        .lines()
        .take_while(|l| *l != BLOCK_END)
        .filter_map(|l| l.trim().strip_prefix("source"))
        .filter_map(|l| l.trim_start().strip_prefix('=').map(|p| PathBuf::from(p.trim())))
        .find(|p| p.parent().is_some_and(|d| d == paths::themes_dir()))
}

/// Returns `config_text` with the managed block replaced (or appended).
pub fn with_managed_block(config_text: &str, theme: Option<&Path>) -> String {
    let block = managed_block(theme);
    if let (Some(s), Some(e)) = (config_text.find(BLOCK_START), config_text.find(BLOCK_END)) {
        let end = e + BLOCK_END.len();
        let end = if config_text[end..].starts_with("\r\n") { end + 2 } else if config_text[end..].starts_with('\n') { end + 1 } else { end };
        format!("{}{}{}", &config_text[..s], block, &config_text[end..])
    } else {
        let sep = if config_text.ends_with('\n') || config_text.is_empty() { "\n" } else { "\n\n" };
        format!("{config_text}{sep}{block}")
    }
}

/// Makes sure the user's config has the managed block and settings.conf exists.
pub fn ensure_managed_block(config: &Path) -> std::io::Result<()> {
    let _ = std::fs::create_dir_all(paths::app_dir());
    if !paths::settings_conf().exists() {
        std::fs::write(paths::settings_conf(), "# Values chosen in hyprwin's Settings window. Edited by the app.\n")?;
    }
    if !paths::binds_conf().exists() {
        std::fs::write(paths::binds_conf(), crate::binds_file::BindsFile::default().render())?;
    }
    let text = std::fs::read_to_string(config).unwrap_or_default();
    if text.contains(BLOCK_START) {
        // Blocks written before binds.conf existed are brought up to date,
        // keeping the selected theme.
        if !text.contains(&paths::binds_conf().display().to_string()) {
            let theme = active_theme(&text);
            std::fs::write(config, with_managed_block(&text, theme.as_deref()))?;
        }
        return Ok(());
    }
    std::fs::write(config, with_managed_block(&text, None))
}

/// Selects a theme (None = no theme) in the user's config.
pub fn apply(config: &Path, theme: Option<&Path>) -> std::io::Result<()> {
    let text = std::fs::read_to_string(config).unwrap_or_default();
    std::fs::write(config, with_managed_block(&text, theme))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_block_is_added_once_and_replaced_in_place() {
        let cfg = "general {\n gaps_in = 1\n}\n";
        let a = with_managed_block(cfg, None);
        assert!(a.starts_with(cfg));
        assert_eq!(a.matches(BLOCK_START).count(), 1);
        let theme = paths::themes_dir().join("nord.conf");
        let b = with_managed_block(&a, Some(&theme));
        assert_eq!(b.matches(BLOCK_START).count(), 1);
        assert_eq!(active_theme(&b), Some(theme));
        let c = with_managed_block(&b, None);
        assert_eq!(active_theme(&c), None);
        assert!(c.starts_with(cfg));
    }

    #[test]
    fn builtins_parse_with_metadata_and_colours() {
        for (stem, text) in BUILTIN {
            let dir = std::env::temp_dir().join("hyprwin-theme-test");
            std::fs::create_dir_all(&dir).unwrap();
            let p = dir.join(format!("{stem}.conf"));
            std::fs::write(&p, text).unwrap();
            let info = read_info(&p).unwrap();
            assert!(!info.name.is_empty() && !info.author.is_empty(), "{stem}");
            assert!(info.active.is_some() && info.inactive.is_some(), "{stem}");
            let cfg = crate::Config::load_str(text);
            assert!(cfg.warnings.is_empty(), "{stem}: {:?}", cfg.warnings);
        }
    }
}
