use windows::Win32::Foundation::{LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    DEVMODEW, ENUM_CURRENT_SETTINGS, EnumDisplayMonitors, EnumDisplaySettingsW, GetMonitorInfoW, HDC, HMONITOR,
    MONITORINFOEXW,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::core::{BOOL, PCWSTR};

use crate::Rect;
use crate::dwm::rect_from;

#[derive(Clone, Debug)]
pub struct Monitor {
    pub handle: isize,
    /// `\\.\DISPLAY1` etc.
    pub device: String,
    pub rect: Rect,
    pub work: Rect,
    pub primary: bool,
    pub dpi: u32,
    pub refresh_hz: u32,
}

impl Monitor {
    pub fn scale(&self) -> f64 {
        self.dpi as f64 / 96.0
    }
}

pub fn monitors() -> Vec<Monitor> {
    unsafe extern "system" fn cb(hmon: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        let out = unsafe { &mut *(data.0 as *mut Vec<HMONITOR>) };
        out.push(hmon);
        true.into()
    }
    let mut handles: Vec<HMONITOR> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(cb), LPARAM(&mut handles as *mut _ as isize));
    }
    handles.into_iter().filter_map(describe).collect()
}

fn describe(h: HMONITOR) -> Option<Monitor> {
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    unsafe { GetMonitorInfoW(h, &mut info.monitorInfo as *mut _).ok().ok()? };
    let device = String::from_utf16_lossy(&info.szDevice).trim_end_matches('\0').to_string();
    let (mut dx, mut dy) = (96u32, 96u32);
    let _ = unsafe { GetDpiForMonitor(h, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) };
    let mut dm = DEVMODEW { dmSize: std::mem::size_of::<DEVMODEW>() as u16, ..Default::default() };
    let dev_w = crate::wide(&device);
    let refresh = if unsafe { EnumDisplaySettingsW(PCWSTR(dev_w.as_ptr()), ENUM_CURRENT_SETTINGS, &mut dm) }.as_bool() {
        dm.dmDisplayFrequency
    } else {
        60
    };
    Some(Monitor {
        handle: h.0 as isize,
        device,
        rect: rect_from(info.monitorInfo.rcMonitor),
        work: rect_from(info.monitorInfo.rcWork),
        primary: info.monitorInfo.dwFlags & 1 != 0,
        dpi: dx,
        refresh_hz: refresh,
    })
}
