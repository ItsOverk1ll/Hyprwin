//! `hyprland.conf` support for hyprwin.
//!
//! Accepts the hyprlang dialects Hyprland users have in their dotfiles
//! (`windowrulev2 = …` from ≤0.47 and `windowrule = match:… , …` / `windowrule { … }`
//! from 0.48–0.54). Unsupported or Wayland-only keys never fail the load. They
//! become warnings, so users can paste a real config and see what was ignored.

pub mod dispatch;
pub mod keys;
pub mod model;
pub mod parse;
pub mod paths;
pub mod rules;
pub mod settings;
pub mod themes;

pub use dispatch::{Cmd, Delta};
pub use keys::{Key, Mods};
pub use model::{AnimEntry, Bind, BindFlags, Config, Color};
pub use rules::{RuleEffect, WindowProps, WindowRule};

/// The config shipped with hyprwin (Hyprland's defaults with Windows apps).
pub const DEFAULT_CONFIG: &str = include_str!("default.conf");
