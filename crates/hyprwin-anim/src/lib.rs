//! Hyprland-compatible animation math: cubic bezier easing, animation styles,
//! and per-frame interpolation of rectangles and opacity.
//!
//! Pure Rust with no Win32. The Win32 layer feeds `progress` from the compositor
//! clock and applies the resulting [`Frame`]s to DWM thumbnails.

mod bezier;
mod style;

pub use bezier::Bezier;
pub use style::{Direction, Frame, Rect, WindowStyle, WorkspaceStyle, window_frame, workspace_frame};

/// Linear interpolation.
#[inline]
pub fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// Hyprland's `SPEED` unit is deciseconds (1 ds = 100 ms).
#[inline]
pub fn speed_to_ms(speed_ds: f64) -> f64 {
    speed_ds * 100.0
}
