//! `settings.conf`: the values chosen in hyprwin's Settings window, stored as
//! plain `category:key = value` hyprlang lines (sourced last by the config).

use std::collections::BTreeMap;

use crate::paths;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SettingsFile {
    pub values: BTreeMap<String, String>,
}

impl SettingsFile {
    pub fn load() -> SettingsFile {
        let text = std::fs::read_to_string(paths::settings_conf()).unwrap_or_default();
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> SettingsFile {
        let p = crate::parse::parse_str(text, "settings.conf");
        SettingsFile { values: p.entries.into_iter().filter(|e| e.block.is_empty()).map(|e| (e.key, e.value)).collect() }
    }

    pub fn render(&self) -> String {
        let mut out = String::from("# Values chosen in hyprwin's Settings window. Edited by the app; safe to delete.\n");
        for (k, v) in &self.values {
            out.push_str(&format!("{k} = {v}\n"));
        }
        out
    }

    pub fn save(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(paths::app_dir())?;
        let tmp = paths::settings_conf().with_extension("conf.tmp");
        std::fs::write(&tmp, self.render())?;
        std::fs::rename(&tmp, paths::settings_conf())
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    pub fn set(&mut self, key: &str, value: impl Into<String>) {
        self.values.insert(key.to_string(), value.into());
    }

    pub fn unset(&mut self, key: &str) {
        self.values.remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_loads_as_config() {
        let mut s = SettingsFile::default();
        s.set("general:gaps_in", "8");
        s.set("general:col.active_border", "rgba(ff0000ff)");
        s.set("hyprwin:force_tile", "true");
        let text = s.render();
        assert_eq!(SettingsFile::parse(&text), s);
        let cfg = crate::Config::load_str(&text);
        assert!(cfg.warnings.is_empty(), "{:?}", cfg.warnings);
        assert!(cfg.force_tile);
        assert_eq!(cfg.wm.gaps_in, hyprwin_layout::Gaps::all(8));
    }
}
