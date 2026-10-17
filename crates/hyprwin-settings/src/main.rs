//! hyprwin Settings: a friendly window for the common options, themes and
//! App Groups. Runs only while open; the tiling daemon stays tiny.
//!
//!   hyprwin-settings [--tab settings|themes|groups|keybinds|about] [--save]

#![windows_subsystem = "windows"]

mod daemon;
mod setup;
mod setup_ui;
mod ui;
mod update;

use eframe::egui;

static TASKBAR_ICO: &[u8] = include_bytes!("../../../assets/icons/taskbar_icon.ico");

fn window_icon() -> Option<egui::IconData> {
    let img = image::load_from_memory_with_format(TASKBAR_ICO, image::ImageFormat::Ico).ok()?.into_rgba8();
    let (w, h) = img.dimensions();
    Some(egui::IconData { rgba: img.into_raw(), width: w, height: h })
}

/// Only one Settings window: a second launch brings the first to the front.
fn already_open() -> bool {
    use std::os::windows::ffi::OsStrExt;
    let name: Vec<u16> = std::ffi::OsStr::new("Local\\hyprwin-settings").encode_wide().chain(std::iter::once(0)).collect();
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateMutexW(attrs: *const core::ffi::c_void, owner: i32, name: *const u16) -> isize;
        fn GetLastError() -> u32;
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn FindWindowW(class: *const u16, title: *const u16) -> isize;
        fn SetForegroundWindow(h: isize) -> i32;
        fn ShowWindow(h: isize, cmd: i32) -> i32;
    }
    unsafe {
        CreateMutexW(std::ptr::null(), 1, name.as_ptr());
        if GetLastError() == 183 {
            let title: Vec<u16> = std::ffi::OsStr::new(ui::TITLE).encode_wide().chain(std::iter::once(0)).collect();
            let h = FindWindowW(std::ptr::null(), title.as_ptr());
            if h != 0 {
                ShowWindow(h, 9); // SW_RESTORE
                SetForegroundWindow(h);
            }
            return true;
        }
    }
    false
}

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().collect();
    if let Some(name) = args.iter().position(|a| a == "--apply-theme").and_then(|i| args.get(i + 1)) {
        let code = match ui::SettingsApp::apply_theme_cli(name) {
            Ok(_) => 0,
            Err(_) => 1,
        };
        std::process::exit(code);
    }
    // Setup mode: run as hyprwin-setup.exe, or with --setup / --install / --uninstall.
    let exe_name = std::env::current_exe().ok().and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_lowercase())).unwrap_or_default();
    let quiet = args.iter().any(|a| a == "--quiet");
    let uninstall = args.iter().any(|a| a == "--uninstall");
    let install = args.iter().any(|a| a == "--install");
    if quiet && (install || uninstall) {
        let r = if uninstall {
            setup::uninstall(args.iter().any(|a| a == "--purge"))
        } else {
            setup::install(&setup::InstallOptions { autostart: args.iter().any(|a| a == "--autostart") })
        };
        std::process::exit(if r.is_ok() { 0 } else { 1 });
    }
    if uninstall || install || args.iter().any(|a| a == "--setup") || exe_name.contains("setup") {
        let mut viewport = egui::ViewportBuilder::default().with_title(setup_ui::TITLE).with_inner_size([640.0, 520.0]).with_resizable(false);
        if let Some(icon) = window_icon() {
            viewport = viewport.with_icon(icon);
        }
        let options = eframe::NativeOptions { viewport, ..Default::default() };
        return eframe::run_native("hyprwin-setup", options, Box::new(move |cc| Ok(Box::new(setup_ui::SetupApp::new(cc, uninstall)))));
    }
    if already_open() {
        return Ok(());
    }
    let tab = args.iter().position(|a| a == "--tab").and_then(|i| args.get(i + 1)).cloned().unwrap_or_else(|| "settings".into());
    let save = args.iter().any(|a| a == "--save");

    let mut viewport = egui::ViewportBuilder::default().with_title(ui::TITLE).with_inner_size([980.0, 700.0]).with_min_inner_size([760.0, 520.0]);
    if let Some(icon) = window_icon() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native("hyprwin-settings", options, Box::new(move |cc| Ok(Box::new(ui::SettingsApp::new(cc, &tab, save)))))
}
