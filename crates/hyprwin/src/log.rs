//! Minimal structured file logger (no dependencies).
//!
//! `%LOCALAPPDATA%\hyprwin\hyprwin.log`, rotated to `.old` above 5 MB.
//! Level from `HYPRWIN_LOG` (error|warn|info|debug|trace), default `info`.

use std::fmt::Arguments;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static FILE: Mutex<Option<File>> = Mutex::new(None);
static LEVEL: AtomicU8 = AtomicU8::new(2);

pub const ERROR: u8 = 0;
pub const WARN: u8 = 1;
pub const INFO: u8 = 2;
pub const DEBUG: u8 = 3;
pub const TRACE: u8 = 4;

pub fn init() -> std::path::PathBuf {
    let dir = hyprwin_win32::ledger::state_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("hyprwin.log");
    if std::fs::metadata(&path).map(|m| m.len() > 5 * 1024 * 1024).unwrap_or(false) {
        let _ = std::fs::rename(&path, dir.join("hyprwin.log.old"));
    }
    let lvl = match std::env::var("HYPRWIN_LOG").unwrap_or_default().to_lowercase().as_str() {
        "error" => ERROR,
        "warn" => WARN,
        "debug" => DEBUG,
        "trace" => TRACE,
        _ => INFO,
    };
    LEVEL.store(lvl, Ordering::Relaxed);
    *FILE.lock().unwrap() = OpenOptions::new().create(true).append(true).open(&path).ok();
    path
}


#[inline]
pub fn enabled(level: u8) -> bool {
    level <= LEVEL.load(Ordering::Relaxed)
}

pub fn write(level: u8, args: Arguments) {
    if !enabled(level) {
        return;
    }
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let (secs, ms) = (ts.as_secs(), ts.subsec_millis());
    let (h, m, s) = ((secs / 3600) % 24, (secs / 60) % 60, secs % 60);
    let name = ["ERROR", "WARN", "INFO", "DEBUG", "TRACE"][level as usize];
    let line = format!("{h:02}:{m:02}:{s:02}.{ms:03}Z {name:<5} {args}\n");
    if let Ok(mut g) = FILE.lock() {
        if let Some(f) = g.as_mut() {
            let _ = f.write_all(line.as_bytes());
        }
    }
    if cfg!(debug_assertions) {
        eprint!("{line}");
    }
}

#[macro_export]
macro_rules! error { ($($t:tt)*) => { $crate::log::write($crate::log::ERROR, format_args!($($t)*)) } }
#[macro_export]
macro_rules! warn { ($($t:tt)*) => { $crate::log::write($crate::log::WARN, format_args!($($t)*)) } }
#[macro_export]
macro_rules! info { ($($t:tt)*) => { $crate::log::write($crate::log::INFO, format_args!($($t)*)) } }
#[macro_export]
macro_rules! debug { ($($t:tt)*) => { if $crate::log::enabled($crate::log::DEBUG) { $crate::log::write($crate::log::DEBUG, format_args!($($t)*)) } } }
#[macro_export]
macro_rules! trace { ($($t:tt)*) => { if $crate::log::enabled($crate::log::TRACE) { $crate::log::write($crate::log::TRACE, format_args!($($t)*)) } } }
