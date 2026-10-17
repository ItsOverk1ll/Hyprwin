//! Hyprland-style borders that DWM can't draw: thicker than ~2 px, or a
//! gradient (`col.active_border = rgba(33ccffee) rgba(00ff99ee) 45deg`).
//!
//! Drawn for the focused window only, as four thin click-through strips
//! around it (top, bottom, left, right). Four strips instead of one big
//! window keep the memory cost tiny: a full-screen layered window would hold
//! a ~15 MB bitmap. The strips sit in z-order directly above the window, so
//! floating windows above it still cover them. Other windows keep DWM's
//! native border in `col.inactive_border`.

use std::sync::OnceLock;

use hyprwin_config::Gradient;
#[cfg(test)]
use hyprwin_config::Color;
use hyprwin_layout::Rect;
use hyprwin_win32::{HWND, wide, window};
use windows::Win32::Foundation::{COLORREF, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC,
    DeleteObject, HGDIOBJ, SelectObject,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::PCWSTR;

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCHITTEST {
        return LRESULT(HTTRANSPARENT as isize);
    }
    unsafe { DefWindowProcW(h, msg, wp, lp) }
}

fn class() -> PCWSTR {
    static C: OnceLock<Vec<u16>> = OnceLock::new();
    let n = C.get_or_init(|| {
        let n = wide("hyprwin.border");
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

/// What to draw, in physical pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Style {
    pub width: i32,
    pub radius: i32,
    pub gradient: Gradient,
}

/// True when the config asks for a border DWM can't draw.
pub fn needed(border_size: i32, g: &Gradient) -> bool {
    border_size > 2 || g.colors.len() > 1
}

/// Physical radius of a window's DWM rounding for Hyprland's `rounding`.
pub fn corner_radius(rounding: u32, scale: f64) -> i32 {
    let logical = match rounding {
        0 => 0.0,
        1..=5 => 4.0,
        _ => 8.0,
    };
    (logical * scale).round() as i32
}

pub fn scale_of(h: HWND) -> f64 {
    let dpi = unsafe { GetDpiForWindow(h) };
    if dpi == 0 { 1.0 } else { dpi as f64 / 96.0 }
}

/// Signed distance from (px, py) to a rounded rectangle (negative inside).
fn sdf(px: f64, py: f64, r: (f64, f64, f64, f64), radius: f64) -> f64 {
    let (x, y, w, h) = r;
    let cx = x + w / 2.0;
    let cy = y + h / 2.0;
    let qx = (px - cx).abs() - (w / 2.0 - radius);
    let qy = (py - cy).abs() - (h / 2.0 - radius);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) - radius
}

fn channel(c: u32, shift: u32) -> f64 {
    ((c >> shift) & 0xFF) as f64
}

/// Colour of the gradient at fraction `t` (0..1), as (a, r, g, b) 0..255.
fn sample(g: &Gradient, t: f64) -> (f64, f64, f64, f64) {
    let cs = &g.colors;
    if cs.len() == 1 {
        let c = cs[0].0;
        return (channel(c, 24), channel(c, 16), channel(c, 8), channel(c, 0));
    }
    let pos = t.clamp(0.0, 1.0) * (cs.len() - 1) as f64;
    let i = (pos.floor() as usize).min(cs.len() - 2);
    let f = pos - i as f64;
    let (a, b) = (cs[i].0, cs[i + 1].0);
    let mix = |s: u32| channel(a, s) + (channel(b, s) - channel(a, s)) * f;
    (mix(24), mix(16), mix(8), mix(0))
}

/// Renders the part of the ring inside `strip` (screen coordinates) into
/// premultiplied BGRA pixels.
fn render(strip: Rect, frame: Rect, st: &Style, out: &mut [u32]) {
    let w = st.width as f64;
    let inner = (frame.x as f64, frame.y as f64, frame.w as f64, frame.h as f64);
    let outer = (inner.0 - w, inner.1 - w, inner.2 + 2.0 * w, inner.3 + 2.0 * w);
    let rin = st.radius as f64;
    let rout = if st.radius > 0 { rin + w } else { 0.0 };
    // Gradient axis through the outer box; Hyprland's angle: 0 = left to right.
    let a = st.gradient.angle_deg.to_radians();
    let (dx, dy) = (a.cos(), -a.sin());
    let (cx, cy) = (outer.0 + outer.2 / 2.0, outer.1 + outer.3 / 2.0);
    let half = (outer.2 * dx.abs() + outer.3 * dy.abs()) / 2.0;
    for row in 0..strip.h {
        let py = (strip.y + row) as f64 + 0.5;
        for col in 0..strip.w {
            let px = (strip.x + col) as f64 + 0.5;
            let d_out = sdf(px, py, outer, rout);
            let d_in = sdf(px, py, inner, rin);
            let cov = (0.5 - d_out).clamp(0.0, 1.0) * (0.5 + d_in).clamp(0.0, 1.0);
            let idx = (row * strip.w + col) as usize;
            if cov <= 0.0 {
                out[idx] = 0;
                continue;
            }
            let t = if half > 0.0 { ((px - cx) * dx + (py - cy) * dy) / (2.0 * half) + 0.5 } else { 0.0 };
            let (ca, r, g, b) = sample(&st.gradient, t);
            let alpha = ca / 255.0 * cov;
            let pm = |v: f64| (v * alpha).round() as u32;
            out[idx] = ((alpha * 255.0).round() as u32) << 24 | pm(r) << 16 | pm(g) << 8 | pm(b);
        }
    }
}

fn draw(h: HWND, r: Rect, frame: Rect, st: &Style) {
    if r.w <= 0 || r.h <= 0 {
        return;
    }
    unsafe {
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: r.w,
                biHeight: -r.h, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mem = CreateCompatibleDC(None);
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let Ok(dib) = CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) else {
            let _ = DeleteDC(mem);
            return;
        };
        let px = std::slice::from_raw_parts_mut(bits as *mut u32, (r.w * r.h) as usize);
        render(r, frame, st, px);
        let old = SelectObject(mem, HGDIOBJ(dib.0));
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
        let pos = POINT { x: r.x, y: r.y };
        let size = SIZE { cx: r.w, cy: r.h };
        let src = POINT { x: 0, y: 0 };
        let _ = UpdateLayeredWindow(h, None, Some(&pos), Some(&size), Some(mem), Some(&src), COLORREF(0), Some(&blend), ULW_ALPHA);
        SelectObject(mem, old);
        let _ = DeleteObject(HGDIOBJ(dib.0));
        let _ = DeleteDC(mem);
    }
}

/// The four strips of the ring around `frame` (screen coordinates).
fn strips(frame: Rect, st: &Style) -> [Rect; 4] {
    let w = st.width;
    let cap = (w + st.radius).max(w); // corners live in the top/bottom strips
    let outer = Rect::new(frame.x - w, frame.y - w, frame.w + 2 * w, frame.h + 2 * w);
    let cap = cap.min(outer.h / 2);
    [
        Rect::new(outer.x, outer.y, outer.w, cap),
        Rect::new(outer.x, outer.bottom() - cap, outer.w, cap),
        Rect::new(outer.x, outer.y + cap, w, (outer.h - 2 * cap).max(0)),
        Rect::new(outer.right() - w, outer.y + cap, w, (outer.h - 2 * cap).max(0)),
    ]
}

#[derive(Default)]
pub struct ActiveBorder {
    wins: Vec<HWND>,
    /// What is on screen: (target, frame, style).
    shown: Option<(isize, Rect, Style)>,
}

impl ActiveBorder {
    fn ensure_windows(&mut self) {
        if !self.wins.is_empty() {
            return;
        }
        for _ in 0..4 {
            let ex = WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
            if let Ok(h) = unsafe { CreateWindowExW(ex, class(), PCWSTR::null(), WS_POPUP, 0, 0, 1, 1, None, None, None, None) } {
                self.wins.push(h);
            }
        }
    }

    pub fn is_ours(&self, h: HWND) -> bool {
        self.wins.contains(&h)
    }

    pub fn hide(&mut self) {
        // Unconditional: cheap, and never trusts our own bookkeeping to be
        // in step with what is on screen.
        if self.shown.take().is_some() {
            crate::debug!("border: hide");
        }
        for &h in &self.wins {
            unsafe {
                if IsWindowVisible(h).as_bool() {
                    let _ = ShowWindow(h, SW_HIDE);
                }
            }
        }
    }

    /// Shows the ring around `target` (whose visible frame is `frame`).
    pub fn show(&mut self, target: HWND, frame: Rect, st: Style) {
        self.ensure_windows();
        if self.wins.len() != 4 {
            return;
        }
        let key = target.0 as isize;
        let same = self.shown.as_ref().is_some_and(|(t, f, s)| *t == key && *f == frame && *s == st);
        // The ring's pixels depend only on the frame's size, not where it is:
        // a dragged window, or focus moving between equal tiles, only moves
        // the strips (rendering them is per-pixel work).
        let same_pixels = self.shown.as_ref().is_some_and(|(_, f, s)| f.w == frame.w && f.h == frame.h && *s == st);
        let parts = strips(frame, &st);
        if !same_pixels {
            for (h, r) in self.wins.iter().zip(parts.iter()) {
                draw(*h, *r, frame, &st);
            }
        }
        self.place_in_z(target, &parts, same_pixels && !same);
        if !same {
            crate::debug!("border: around {:#x} at {:?}", key, frame);
        }
        self.shown = Some((key, frame, st));
    }

    /// Directly above the target: anything above it (a floating window)
    /// still covers the border. `reposition` also moves the strips to `parts`.
    fn place_in_z(&self, target: HWND, parts: &[Rect; 4], reposition: bool) {
        let topmost = |h: HWND| window::ex_style(h) & WS_EX_TOPMOST.0 != 0;
        let mut above = unsafe { GetWindow(target, GW_HWNDPREV) }.ok();
        while let Some(a) = above {
            if !self.is_ours(a) {
                break;
            }
            above = unsafe { GetWindow(a, GW_HWNDPREV) }.ok();
        }
        // Inserting below a topmost window would make ours topmost too.
        let after = match above {
            Some(a) if !topmost(a) => a,
            _ => HWND_TOP,
        };
        unsafe {
            for (h, r) in self.wins.iter().zip(parts.iter()) {
                let mv = if reposition { SET_WINDOW_POS_FLAGS(0) } else { SWP_NOMOVE };
                let flags = SWP_NOACTIVATE | SWP_NOSIZE | mv | if r.w > 0 && r.h > 0 { SWP_SHOWWINDOW } else { SWP_HIDEWINDOW };
                let _ = SetWindowPos(*h, Some(after), r.x, r.y, 0, 0, flags);
                if topmost(*h) {
                    let _ = SetWindowPos(*h, Some(HWND_NOTOPMOST), 0, 0, 0, 0, SWP_NOACTIVATE | SWP_NOSIZE | SWP_NOMOVE);
                }
            }
        }
    }
}

impl Drop for ActiveBorder {
    fn drop(&mut self) {
        for &h in &self.wins {
            unsafe {
                let _ = DestroyWindow(h);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(width: i32, radius: i32, colors: Vec<u32>, angle: f64) -> Style {
        Style { width, radius, gradient: Gradient { colors: colors.into_iter().map(Color).collect(), angle_deg: angle } }
    }

    #[test]
    fn ring_covers_only_the_border_band() {
        let frame = Rect::new(100, 100, 400, 300);
        let s = st(4, 0, vec![0xFF_FF0000], 0.0);
        let top = strips(frame, &s)[0];
        let mut px = vec![0u32; (top.w * top.h) as usize];
        render(top, frame, &s, &mut px);
        // Top strip is the 4 rows above the frame, full opacity red.
        assert_eq!(top, Rect::new(96, 96, 408, 4));
        assert!(px.iter().all(|&p| p == 0xFFFF0000), "{:x?}", &px[..4]);
    }

    #[test]
    fn rounded_corner_is_transparent_and_antialiased() {
        let frame = Rect::new(0, 0, 200, 200);
        let s = st(6, 12, vec![0xFF_00FF00], 0.0);
        let top = strips(frame, &s)[0];
        let mut px = vec![0u32; (top.w * top.h) as usize];
        render(top, frame, &s, &mut px);
        assert_eq!(px[0] >> 24, 0, "outer corner pixel is outside the rounded ring");
        let mid = px[(top.w / 2) as usize] >> 24;
        assert_eq!(mid, 255, "top edge middle is solid");
        assert!(px.iter().any(|&p| { let a = p >> 24; a > 0 && a < 255 }), "edges are anti-aliased");
    }

    #[test]
    fn gradient_runs_left_to_right_at_zero_degrees() {
        let frame = Rect::new(0, 0, 1000, 100);
        let s = st(4, 0, vec![0xFF_0000FF, 0xFF_FF0000], 0.0);
        let top = strips(frame, &s)[0];
        let mut px = vec![0u32; (top.w * top.h) as usize];
        render(top, frame, &s, &mut px);
        let left = px[0];
        let right = px[(top.w - 1) as usize];
        assert!(left & 0xFF > 0xF0 && (left >> 16) & 0xFF < 0x10, "left is blue: {left:08x}");
        assert!((right >> 16) & 0xFF > 0xF0 && right & 0xFF < 0x10, "right is red: {right:08x}");
    }

    #[test]
    fn pixels_do_not_depend_on_position() {
        // `show` only moves the strips when the frame moves without resizing.
        let s = st(5, 12, vec![0xFF_0000FF, 0xEE_FF8800, 0xFF_00FF00], 37.0);
        let a = Rect::new(10, 20, 640, 480);
        let b = Rect::new(-1913, 777, 640, 480);
        for (sa, sb) in strips(a, &s).into_iter().zip(strips(b, &s)) {
            let mut pa = vec![0u32; (sa.w * sa.h) as usize];
            let mut pb = vec![0u32; (sb.w * sb.h) as usize];
            render(sa, a, &s, &mut pa);
            render(sb, b, &s, &mut pb);
            assert_eq!(pa, pb);
        }
    }

    #[test]
    fn needed_only_for_what_dwm_cannot_draw() {
        let solid = Gradient::solid(Color(0xFF33CCFF));
        assert!(!needed(2, &solid));
        assert!(needed(4, &solid));
        assert!(needed(2, &Gradient { colors: vec![Color(1), Color(2)], angle_deg: 45.0 }));
    }
}
