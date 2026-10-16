//! Animation surfaces: live DWM thumbnails of real windows drawn into windows
//! we own, so we can animate "the window" at the compositor's frame rate while
//! the real window is moved exactly once.
//!
//! Two strategies, chosen by the animation spike:
//! - [`Ghost`]: one small top-level window per animated source, moved per frame.
//! - [`Overlay`]: one click-through, colour-keyed window per monitor holding
//!   many thumbnails; per frame only the thumbnail rects change.

use std::sync::OnceLock;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWM_THUMBNAIL_PROPERTIES, DWM_TNP_OPACITY, DWM_TNP_RECTDESTINATION, DWM_TNP_RECTSOURCE,
    DWM_TNP_SOURCECLIENTAREAONLY, DWM_TNP_VISIBLE, DwmRegisterThumbnail, DwmUnregisterThumbnail,
    DwmUpdateThumbnailProperties,
};
use windows::Win32::Graphics::Gdi::{CreateSolidBrush, HBRUSH};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    BeginDeferWindowPos, CreateWindowExW, DefWindowProcW, DeferWindowPos, DestroyWindow, EndDeferWindowPos,
    HTTRANSPARENT, HWND_TOPMOST, LWA_ALPHA, LWA_COLORKEY, RegisterClassW, SW_SHOWNOACTIVATE, SWP_NOACTIVATE,
    SWP_NOOWNERZORDER, SWP_NOZORDER, SetLayeredWindowAttributes, SetWindowPos, ShowWindow, WINDOW_EX_STYLE,
    WM_NCHITTEST, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
    WS_POPUP,
};
use windows::core::PCWSTR;

use crate::Rect;

/// Key colour for overlay transparency. Deliberately odd so it never matches real content.
const KEY_RGB: u32 = 0x00_01_00_01; // COLORREF 0x00BBGGRR

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCHITTEST {
        return LRESULT(HTTRANSPARENT as isize);
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

fn class_name() -> PCWSTR {
    static CLASS: OnceLock<Vec<u16>> = OnceLock::new();
    let name = CLASS.get_or_init(|| {
        let name = crate::wide("hyprwin.ghost");
        let brush: HBRUSH = unsafe { CreateSolidBrush(COLORREF(KEY_RGB)) };
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: unsafe { GetModuleHandleW(None).unwrap_or_default().into() },
            lpszClassName: PCWSTR(name.as_ptr()),
            hbrBackground: brush,
            ..Default::default()
        };
        unsafe { RegisterClassW(&wc) };
        name
    });
    PCWSTR(name.as_ptr())
}

fn create_surface(r: Rect, layered: Option<LayerMode>) -> windows::core::Result<HWND> {
    let mut ex: WINDOW_EX_STYLE = WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE;
    if layered.is_some() {
        ex |= WS_EX_LAYERED | WS_EX_TRANSPARENT;
    }
    let hwnd = unsafe {
        CreateWindowExW(ex, class_name(), PCWSTR::null(), WS_POPUP, r.x, r.y, r.w, r.h, None, None, None, None)?
    };
    unsafe {
        match layered {
            Some(LayerMode::ColorKey) => SetLayeredWindowAttributes(hwnd, COLORREF(KEY_RGB), 0, LWA_COLORKEY)?,
            Some(LayerMode::Opaque) => SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA)?,
            None => {}
        }
    }
    Ok(hwnd)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerMode {
    /// Pixels of the key colour are transparent and click-through.
    ColorKey,
    /// Whole window opaque but click-through.
    Opaque,
}

/// RAII DWM thumbnail registration.
pub struct Thumbnail {
    id: isize,
}

impl Thumbnail {
    pub fn register(dest: HWND, source: HWND) -> windows::core::Result<Self> {
        let id = unsafe { DwmRegisterThumbnail(dest, source)? };
        Ok(Self { id })
    }

    /// `dest` is relative to the destination window's client area. `source`
    /// crops the source window (window-relative coordinates); `None` = whole window.
    pub fn update(&self, dest: Rect, source: Option<Rect>, opacity: u8, visible: bool) -> windows::core::Result<()> {
        let mut p = DWM_THUMBNAIL_PROPERTIES {
            dwFlags: DWM_TNP_RECTDESTINATION | DWM_TNP_OPACITY | DWM_TNP_VISIBLE | DWM_TNP_SOURCECLIENTAREAONLY,
            rcDestination: to_win_rect(dest),
            opacity,
            fVisible: visible.into(),
            fSourceClientAreaOnly: false.into(),
            ..Default::default()
        };
        if let Some(s) = source {
            p.dwFlags |= DWM_TNP_RECTSOURCE;
            p.rcSource = to_win_rect(s);
        }
        unsafe { DwmUpdateThumbnailProperties(self.id, &p) }
    }
}

impl Drop for Thumbnail {
    fn drop(&mut self) {
        let _ = unsafe { DwmUnregisterThumbnail(self.id) };
    }
}

fn to_win_rect(r: Rect) -> RECT {
    RECT { left: r.x, top: r.y, right: r.right(), bottom: r.bottom() }
}

/// Per-window ghost: a top-level window exactly the size of the animated frame.
pub struct Ghost {
    pub hwnd: HWND,
    thumb: Thumbnail,
    source_crop: Option<Rect>,
}

impl Ghost {
    pub fn new(source: HWND, start: Rect, source_crop: Option<Rect>, layered: bool) -> windows::core::Result<Self> {
        let hwnd = create_surface(start, layered.then_some(LayerMode::Opaque))?;
        let thumb = match Thumbnail::register(hwnd, source) {
            Ok(t) => t,
            Err(e) => {
                let _ = unsafe { DestroyWindow(hwnd) };
                return Err(e);
            }
        };
        let g = Self { hwnd, thumb, source_crop };
        g.thumb.update(Rect::new(0, 0, start.w, start.h), source_crop, 255, true)?;
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, windows::Win32::UI::WindowsAndMessaging::SWP_NOMOVE | windows::Win32::UI::WindowsAndMessaging::SWP_NOSIZE | SWP_NOACTIVATE);
        }
        Ok(g)
    }
}

impl Drop for Ghost {
    fn drop(&mut self) {
        let _ = unsafe { DestroyWindow(self.hwnd) };
    }
}

/// Moves many ghosts atomically in one compositor update.
pub fn apply_ghost_frames(frames: &[(&Ghost, Rect, u8)]) -> windows::core::Result<()> {
    unsafe {
        let mut hdwp = BeginDeferWindowPos(frames.len() as i32)?;
        for (g, r, _) in frames {
            hdwp = DeferWindowPos(hdwp, g.hwnd, None, r.x, r.y, r.w, r.h, SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER)?;
        }
        EndDeferWindowPos(hdwp)?;
    }
    for (g, r, a) in frames {
        g.thumb.update(Rect::new(0, 0, r.w, r.h), g.source_crop, *a, true)?;
    }
    Ok(())
}

/// One overlay per monitor holding any number of thumbnails.
pub struct Overlay {
    pub hwnd: HWND,
    pub origin: Rect,
    thumbs: Vec<(Thumbnail, Option<Rect>)>,
}

impl Overlay {
    pub fn new(monitor: Rect, mode: LayerMode) -> windows::core::Result<Self> {
        let hwnd = create_surface(monitor, Some(mode))?;
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        Ok(Self { hwnd, origin: monitor, thumbs: Vec::new() })
    }

    /// Adds a thumbnail; later additions draw on top. Returns its index.
    pub fn add(&mut self, source: HWND, source_crop: Option<Rect>) -> windows::core::Result<usize> {
        self.thumbs.push((Thumbnail::register(self.hwnd, source)?, source_crop));
        Ok(self.thumbs.len() - 1)
    }

    /// `screen` is in screen coordinates.
    pub fn set(&self, idx: usize, screen: Rect, opacity: u8) -> windows::core::Result<()> {
        let (t, crop) = &self.thumbs[idx];
        let local = Rect::new(screen.x - self.origin.x, screen.y - self.origin.y, screen.w, screen.h);
        t.update(local, *crop, opacity, true)
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        self.thumbs.clear();
        let _ = unsafe { DestroyWindow(self.hwnd) };
    }
}
