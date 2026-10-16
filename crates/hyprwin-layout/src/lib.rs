//! The pure hyprwin window-manager model: dwindle and master layouts,
//! per-monitor workspaces, special workspaces, focus, floating, fullscreen and
//! pinning, with Hyprland semantics.
//!
//! No Win32 here. Every operation mutates [`Wm`] and returns [`Effect`]s that
//! the Win32 layer executes, which keeps the whole model unit-testable.

pub mod dwindle;
pub mod geom;
pub mod master;
pub mod wm;

pub use geom::{Dir, Gaps, Rect};
pub use wm::{Effect, FullscreenMode, LayoutKind, Wm, WmConfig, WsSel, remap_layout_ids};

/// Opaque window identity (the HWND value in the daemon).
pub type WinId = u64;
/// Monitor index into `Wm::monitors`.
pub type MonId = usize;
/// Workspace id. 1.. are regular workspaces; negative ids are special workspaces.
pub type WsId = i64;
