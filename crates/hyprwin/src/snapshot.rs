//! Pictures of windows that are about to close, for `windowsOut` animations.
//!
//! A closed window leaves nothing for a DWM thumbnail to show, so before
//! hyprwin closes one (`killactive`, `closewindow`) it captures it with
//! `PrintWindow` and puts the picture in a window of its own, exactly over
//! the original. When the original disappears, the animation engine shows a
//! thumbnail of *that* window shrinking and fading away.

use std::sync::OnceLock;
use std::sync::mpsc::channel;
use std::time::Duration;

use hyprwin_layout::Rect;
use hyprwin_win32::{HWND, dwm, wide, window};
use windows::Win32::Foundation::{COLORREF, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC,
    DeleteObject, HGDIOBJ, SelectObject,
};
use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::PCWSTR;

/// PW_RENDERFULLCONTENT: capture DirectX/Chromium content too.
const PW_RENDERFULLCONTENT: u32 = 2;
/// Longest we wait for an app to paint its picture (a hung app must not
/// stall hyprwin).
const CAPTURE_TIMEOUT: Duration = Duration::from_millis(250);

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCHITTEST {
        return LRESULT(HTTRANSPARENT as isize);
    }
    unsafe { DefWindowProcW(h, msg, wp, lp) }
}

fn class() -> PCWSTR {
    static C: OnceLock<Vec<u16>> = OnceLock::new();
    let n = C.get_or_init(|| {
        let n = wide("hyprwin.snapshot");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: unsafe { GetModuleHandleW(None).unwrap_or_default().into() },
            lpszClassName: PCWSTR(n.as_ptr()),
            ..Default::default()
        };
        unsafe { RegisterClassW(&wc) };
        n
    });
    PCWSTR(n.as_ptr())
}

/// Premultiplied BGRA pixels of a window's visible frame.
pub struct Picture {
    pub frame: Rect,
    pixels: Vec<u32>,
}

/// Captures `h` on a worker thread; `None` if it fails or takes too long.
/// `radius` rounds the corners like DWM does (physical px).
pub fn capture(h: HWND, radius: i32) -> Option<Picture> {
    let raw = h.0 as isize;
    let (tx, rx) = channel();
    std::thread::Builder::new()
        .name("hyprwin-snapshot".into())
        .spawn(move || {
            let _ = tx.send(capture_now(HWND(raw as *mut _), radius));
        })
        .ok()?;
    rx.recv_timeout(CAPTURE_TIMEOUT).ok().flatten()
}

fn capture_now(h: HWND, radius: i32) -> Option<Picture> {
    let wr = window::window_rect(h).ok()?;
    let fb = dwm::frame_bounds(h).ok()?;
    if wr.w <= 0 || wr.h <= 0 || fb.w <= 0 || fb.h <= 0 || wr.w * wr.h > 64_000_000 {
        return None;
    }
    unsafe {
        let bmi = bitmap_info(wr.w, wr.h);
        let mem = CreateCompatibleDC(None);
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let Ok(dib) = CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) else {
            let _ = DeleteDC(mem);
            return None;
        };
        let old = SelectObject(mem, HGDIOBJ(dib.0));
        let ok = PrintWindow(h, mem, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT)).as_bool();
        let src = std::slice::from_raw_parts(bits as *const u32, (wr.w * wr.h) as usize);
        let (ox, oy) = (fb.x - wr.x, fb.y - wr.y);
        let mut pixels = vec![0u32; (fb.w * fb.h) as usize];
        if ok {
            let r = radius as f64;
            for y in 0..fb.h {
                let sy = y + oy;
                if sy < 0 || sy >= wr.h {
                    continue;
                }
                for x in 0..fb.w {
                    let sx = x + ox;
                    if sx < 0 || sx >= wr.w {
                        continue;
                    }
                    // PrintWindow leaves alpha undefined: the frame is opaque,
                    // except outside DWM's rounded corners.
                    let cov = if r > 0.0 { corner_coverage(x as f64 + 0.5, y as f64 + 0.5, fb.w as f64, fb.h as f64, r) } else { 1.0 };
                    let p = src[(sy * wr.w + sx) as usize] & 0x00FF_FFFF;
                    pixels[(y * fb.w + x) as usize] = premultiply(p, cov);
                }
            }
        }
        SelectObject(mem, old);
        let _ = DeleteObject(HGDIOBJ(dib.0));
        let _ = DeleteDC(mem);
        ok.then_some(Picture { frame: fb, pixels })
    }
}

fn bitmap_info(w: i32, h: i32) -> BITMAPINFO {
    BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Coverage (0..1) of a pixel centre by a w x h rounded rectangle at the origin.
fn corner_coverage(px: f64, py: f64, w: f64, h: f64, r: f64) -> f64 {
    let qx = (px - w / 2.0).abs() - (w / 2.0 - r);
    let qy = (py - h / 2.0).abs() - (h / 2.0 - r);
    let d = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r;
    (0.5 - d).clamp(0.0, 1.0)
}

fn premultiply(rgb: u32, a: f64) -> u32 {
    if a >= 1.0 {
        return 0xFF00_0000 | rgb;
    }
    let ch = |s: u32| (((rgb >> s) & 0xFF) as f64 * a).round() as u32;
    ((a * 255.0).round() as u32) << 24 | ch(16) << 16 | ch(8) << 8 | ch(0)
}

/// Shows `pic` in a click-through window directly above `above` (the window
/// it is a picture of). Returns the window.
pub fn show(pic: &Picture, above: HWND) -> Option<HWND> {
    let f = pic.frame;
    let ex = WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
    unsafe {
        let h = CreateWindowExW(ex, class(), PCWSTR::null(), WS_POPUP, f.x, f.y, f.w, f.h, None, None, None, None).ok()?;
        let bmi = bitmap_info(f.w, f.h);
        let mem = CreateCompatibleDC(None);
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        if let Ok(dib) = CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
            std::ptr::copy_nonoverlapping(pic.pixels.as_ptr(), bits as *mut u32, pic.pixels.len());
            let old = SelectObject(mem, HGDIOBJ(dib.0));
            let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
            let _ = UpdateLayeredWindow(h, None, Some(&POINT { x: f.x, y: f.y }), Some(&SIZE { cx: f.w, cy: f.h }), Some(mem), Some(&POINT { x: 0, y: 0 }), COLORREF(0), Some(&blend), ULW_ALPHA);
            SelectObject(mem, old);
            let _ = DeleteObject(HGDIOBJ(dib.0));
        }
        let _ = DeleteDC(mem);
        // Same z-slot as the window it stands in for.
        let prev = GetWindow(above, GW_HWNDPREV).ok().filter(|p| window::ex_style(*p) & WS_EX_TOPMOST.0 == 0);
        let _ = SetWindowPos(h, Some(prev.unwrap_or(HWND_TOP)), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
        if window::ex_style(h) & WS_EX_TOPMOST.0 != 0 {
            let _ = SetWindowPos(h, Some(HWND_NOTOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
        Some(h)
    }
}

pub fn destroy(h: HWND) {
    unsafe {
        let _ = DestroyWindow(h);
    }
}

/// Hides one of our own windows from the screen while DWM thumbnails of it
/// keep rendering (DWMWA_CLOAK works on windows of the calling process).
pub fn cloak_own(h: HWND) {
    use windows::Win32::Graphics::Dwm::{DWMWA_CLOAK, DwmSetWindowAttribute};
    let on: i32 = 1;
    unsafe {
        let _ = DwmSetWindowAttribute(h, DWMWA_CLOAK, &on as *const _ as *const core::ffi::c_void, 4);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners_are_cut_and_edges_kept() {
        assert_eq!(corner_coverage(0.5, 0.5, 100.0, 100.0, 8.0), 0.0);
        assert_eq!(corner_coverage(50.0, 0.5, 100.0, 100.0, 8.0), 1.0);
        assert_eq!(corner_coverage(50.0, 50.0, 100.0, 100.0, 8.0), 1.0);
    }

    #[test]
    fn premultiplied_alpha() {
        assert_eq!(premultiply(0x00FF_8000, 1.0), 0xFFFF_8000);
        assert_eq!(premultiply(0x00FF_8000, 0.5), 0x8080_4000);
    }
}
