use std::ffi::c_void;
use std::mem::size_of;

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DEFAULT,
    DWMWCP_DONOTROUND, DWMWCP_ROUND, DWMWCP_ROUNDSMALL, DwmGetWindowAttribute, DwmSetWindowAttribute,
};

use crate::Rect;

pub fn rect_from(r: RECT) -> Rect {
    Rect::new(r.left, r.top, r.right - r.left, r.bottom - r.top)
}

/// The visible frame (what the user sees), excluding the invisible resize borders.
pub fn frame_bounds(hwnd: HWND) -> windows::core::Result<Rect> {
    let mut r = RECT::default();
    unsafe {
        DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut _ as *mut c_void, size_of::<RECT>() as u32)?
    };
    Ok(rect_from(r))
}

/// Invisible border thickness (left, top, right, bottom) = window rect minus frame bounds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Insets {
    pub l: i32,
    pub t: i32,
    pub r: i32,
    pub b: i32,
}

impl Insets {
    /// Converts a desired visible rect into the outer rect to pass to `SetWindowPos`.
    pub fn outer(&self, visible: Rect) -> Rect {
        Rect::new(visible.x - self.l, visible.y - self.t, visible.w + self.l + self.r, visible.h + self.t + self.b)
    }
}

pub fn invisible_insets(hwnd: HWND) -> windows::core::Result<Insets> {
    let outer = crate::window::window_rect(hwnd)?;
    let vis = frame_bounds(hwnd)?;
    Ok(Insets { l: vis.x - outer.x, t: vis.y - outer.y, r: outer.right() - vis.right(), b: outer.bottom() - vis.bottom() })
}

/// 0 = not cloaked; bit 1 = app, bit 2 = shell, bit 4 = inherited.
pub fn cloaked_state(hwnd: HWND) -> u32 {
    let mut v: u32 = 0;
    let _ = unsafe { DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut v as *mut _ as *mut c_void, 4) };
    v
}

/// Colour as 0xRRGGBB. `None` restores the system default border.
pub fn set_border_color(hwnd: HWND, rgb: Option<u32>) -> windows::core::Result<()> {
    const DWMWA_COLOR_DEFAULT: u32 = 0xFFFF_FFFF;
    let v: u32 = match rgb {
        // COLORREF is 0x00BBGGRR
        Some(c) => ((c & 0xFF) << 16) | (c & 0xFF00) | ((c >> 16) & 0xFF),
        None => DWMWA_COLOR_DEFAULT,
    };
    unsafe { DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR, &v as *const _ as *const c_void, 4) }
}

/// No DWM border at all (hyprwin draws its own).
pub fn set_border_none(hwnd: HWND) -> windows::core::Result<()> {
    const DWMWA_COLOR_NONE: u32 = 0xFFFF_FFFE;
    unsafe { DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR, &DWMWA_COLOR_NONE as *const _ as *const c_void, 4) }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Corners {
    Default,
    Square,
    Round,
    RoundSmall,
}

impl Corners {
    /// Maps Hyprland's `rounding = N` (px) onto the four presets Windows offers.
    pub fn from_hypr_rounding(px: u32) -> Corners {
        match px {
            0 => Corners::Square,
            1..=5 => Corners::RoundSmall,
            _ => Corners::Round,
        }
    }
}

pub fn set_corners(hwnd: HWND, c: Corners) -> windows::core::Result<()> {
    let v = match c {
        Corners::Default => DWMWCP_DEFAULT,
        Corners::Square => DWMWCP_DONOTROUND,
        Corners::Round => DWMWCP_ROUND,
        Corners::RoundSmall => DWMWCP_ROUNDSMALL,
    };
    unsafe { DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &v as *const _ as *const c_void, 4) }
}
