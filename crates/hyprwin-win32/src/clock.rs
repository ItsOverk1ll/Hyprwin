//! Frame pacing locked to the compositor, so animations run at the monitor's
//! real refresh rate (240 Hz panels get 240 Hz animations) without timers.

use std::time::{Duration, Instant};

use windows::Win32::Graphics::DirectComposition::DCompositionWaitForCompositorClock;
use windows::Win32::Graphics::Dwm::DwmFlush;

pub struct FrameClock {
    use_compositor_clock: bool,
    last: Instant,
}

impl Default for FrameClock {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameClock {
    pub fn new() -> Self {
        // DCompositionWaitForCompositorClock exists on Windows 11+. Probe once.
        let ok = unsafe { DCompositionWaitForCompositorClock(None, 100) } != 0x102; // != WAIT_TIMEOUT
        Self { use_compositor_clock: ok, last: Instant::now() }
    }

    pub fn source(&self) -> &'static str {
        if self.use_compositor_clock { "DCompositionWaitForCompositorClock" } else { "DwmFlush" }
    }

    /// Blocks until the next composition frame. Returns time since the previous call.
    pub fn wait(&mut self) -> Duration {
        unsafe {
            if self.use_compositor_clock {
                DCompositionWaitForCompositorClock(None, 50);
            } else {
                let _ = DwmFlush();
            }
        }
        let now = Instant::now();
        let dt = now - self.last;
        self.last = now;
        dt
    }
}
