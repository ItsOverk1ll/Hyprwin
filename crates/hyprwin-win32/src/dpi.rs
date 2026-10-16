use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};

/// Opts the process into per-monitor-v2 DPI awareness so every coordinate we
/// see and set is in physical pixels. The release manifest declares this too;
/// calling it again is harmless.
pub fn make_per_monitor_aware() {
    let _ = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
}
