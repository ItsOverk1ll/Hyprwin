//! Per-user install and uninstall (no admin rights needed).
//!
//! Install copies hyprwin's programs to `%LOCALAPPDATA%\hyprwin\bin`, adds
//! Start-menu shortcuts, an "Installed apps" entry (so Windows can uninstall
//! it) and, if wanted, starts hyprwin at sign-in. Uninstall stops hyprwin
//! (which puts every window and the taskbar back) and removes all of that;
//! the config, themes and groups are kept unless the user asks otherwise.
//!
//! `HYPRWIN_SETUP_TEST=1` redirects every path and registry name to a
//! sandbox so the live test never touches a real installation.

use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const NO_WINDOW: u32 = 0x0800_0000;
pub const PROGRAMS: &[&str] = &["hyprwin.exe", "hyprwin-guard.exe", "hyprwinctl.exe"];

/// The single-file installer (`--features bundle`, built by scripts/package.ps1)
/// carries every program inside itself, so it needs nothing next to it.
#[cfg(feature = "bundle")]
const BUNDLE: &[(&str, &[u8])] = &[
    ("hyprwin.exe", include_bytes!(concat!(env!("HYPRWIN_BUNDLE_DIR"), "/hyprwin.exe"))),
    ("hyprwin-guard.exe", include_bytes!(concat!(env!("HYPRWIN_BUNDLE_DIR"), "/hyprwin-guard.exe"))),
    ("hyprwinctl.exe", include_bytes!(concat!(env!("HYPRWIN_BUNDLE_DIR"), "/hyprwinctl.exe"))),
    ("hyprwin-settings.exe", include_bytes!(concat!(env!("HYPRWIN_BUNDLE_DIR"), "/hyprwin-settings.exe"))),
];
#[cfg(not(feature = "bundle"))]
const BUNDLE: &[(&str, &[u8])] = &[];

/// True for the single-file installer.
pub fn is_bundle() -> bool {
    !BUNDLE.is_empty()
}

/// Writes `bytes` to `to` (via a temp file and rename), retrying while an
/// old copy is still locked by a process that is exiting.
fn write_retry(bytes: &[u8], to: &Path) -> std::io::Result<()> {
    let tmp = to.with_extension("exe.new");
    std::fs::write(&tmp, bytes)?;
    let t0 = Instant::now();
    loop {
        match std::fs::rename(&tmp, to) {
            Ok(()) => return Ok(()),
            Err(_) if t0.elapsed() < Duration::from_secs(5) => std::thread::sleep(Duration::from_millis(200)),
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(e);
            }
        }
    }
}

fn test_mode() -> bool {
    std::env::var_os("HYPRWIN_SETUP_TEST").is_some()
}

fn local_appdata() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir)
}

fn sandbox() -> PathBuf {
    std::env::temp_dir().join("hyprwin-setup-test")
}

/// Where a new install goes unless the user picks another folder.
pub fn default_dir() -> PathBuf {
    if test_mode() { sandbox().join("bin") } else { local_appdata().join("hyprwin").join("bin") }
}

/// The folder hyprwin is installed in: the one recorded in its Installed-apps
/// entry (the user may have chosen it), else the default.
pub fn bin_dir() -> PathBuf {
    registered_dir().filter(|d| d.join("hyprwin.exe").exists()).unwrap_or_else(default_dir)
}

fn registered_dir() -> Option<PathBuf> {
    let out = std::process::Command::new("reg.exe").args(["query", &uninstall_key(), "/v", "InstallLocation"]).creation_flags(NO_WINDOW).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().find(|l| l.trim_start().starts_with("InstallLocation"))?;
    let value = line.split_once("REG_SZ")?.1.trim();
    (!value.is_empty()).then(|| PathBuf::from(value))
}

/// Checks a folder the user typed or picked, creating it if needed.
pub fn check_dir(dir: &Path) -> Result<(), String> {
    if !dir.is_absolute() || dir.parent().is_none() {
        return Err("Pick a full folder path, such as C:\\Users\\you\\Apps\\hyprwin (not a whole drive).".into());
    }
    let lower = dir.to_string_lossy().to_lowercase();
    if lower.contains("\\windows\\") || lower.ends_with("\\windows") {
        return Err("That's a Windows system folder. Pick another one.".into());
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("Couldn't create that folder ({e}). Pick another one."))?;
    let probe = dir.join(".hyprwin-write-test");
    std::fs::write(&probe, b"ok").map_err(|_| {
        "Windows doesn't let normal programs write there (for example Program Files needs admin rights). Pick a folder in your user folder, or keep the suggested one.".to_string()
    })?;
    let _ = std::fs::remove_file(probe);
    Ok(())
}

/// Free space on the drive holding `dir` (or its nearest existing parent).
pub fn free_space(dir: &Path) -> Option<u64> {
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let mut p = dir;
    while !p.exists() {
        p = p.parent()?;
    }
    let wide: Vec<u16> = p.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let mut free = 0u64;
    unsafe { GetDiskFreeSpaceExW(windows::core::PCWSTR(wide.as_ptr()), Some(&mut free), None, None) }.ok()?;
    Some(free)
}

/// Roughly how much the installed programs take, in bytes.
pub fn install_size() -> u64 {
    if is_bundle() {
        BUNDLE.iter().map(|(_, b)| b.len() as u64).sum()
    } else {
        12 * 1024 * 1024
    }
}

/// Removes hyprwin's own programs from `dir`, then the folder only if it is
/// left empty: the user may have installed into a folder holding other files.
fn remove_programs(dir: &Path, skip: Option<&Path>) {
    for p in PROGRAMS.iter().chain(std::iter::once(&"hyprwin-settings.exe")) {
        let f = dir.join(p);
        if skip.is_some_and(|m| same_dir(m, &f)) {
            continue;
        }
        let _ = std::fs::remove_file(&f);
        let _ = std::fs::remove_file(f.with_extension("exe.new"));
    }
    let _ = std::fs::remove_dir(dir);
}

fn start_menu_dir() -> PathBuf {
    if test_mode() {
        return sandbox().join("startmenu");
    }
    std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_default().join(r"Microsoft\Windows\Start Menu\Programs")
}

fn name() -> &'static str {
    if test_mode() { "hyprwin-test" } else { "hyprwin" }
}

fn uninstall_key() -> String {
    format!(r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\{}", name())
}

const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

fn reg(args: &[&str]) -> bool {
    std::process::Command::new("reg.exe").args(args).creation_flags(NO_WINDOW).output().map(|o| o.status.success()).unwrap_or(false)
}

pub fn is_installed() -> bool {
    bin_dir().join("hyprwin.exe").exists()
}

fn same_dir(a: &Path, b: &Path) -> bool {
    let n = |p: &Path| p.to_string_lossy().trim_end_matches('\\').to_lowercase();
    n(a) == n(b)
}

/// Asks a running hyprwin to exit (it restores every window and the taskbar
/// on the way out) and waits until it has.
pub fn stop_daemon() {
    if test_mode() {
        return;
    }
    if hyprwin_ipc::request("dispatch exit").is_err() {
        return;
    }
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(6) {
        std::thread::sleep(Duration::from_millis(150));
        if hyprwin_ipc::request("status").is_err() {
            break;
        }
    }
    // Give the guard a moment to notice and exit too.
    std::thread::sleep(Duration::from_millis(600));
}

fn copy_retry(from: &Path, to: &Path) -> std::io::Result<()> {
    let t0 = Instant::now();
    loop {
        match std::fs::copy(from, to) {
            Ok(_) => return Ok(()),
            Err(e) if t0.elapsed() < Duration::from_secs(5) => {
                let _ = e;
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(e) => return Err(e),
        }
    }
}

fn shortcut(lnk: &Path, target: &Path, args: &str, description: &str) -> bool {
    // WScript.Shell is the simplest dependable way to write a .lnk.
    let ps = format!(
        "$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{}');$s.TargetPath='{}';$s.Arguments='{}';$s.WorkingDirectory='{}';$s.Description='{}';$s.IconLocation='{},0';$s.Save()",
        lnk.display(),
        target.display(),
        args,
        target.parent().map(|p| p.display().to_string()).unwrap_or_default(),
        description,
        target.display()
    );
    std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", &ps])
        .creation_flags(NO_WINDOW)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[derive(Clone, Debug)]
pub struct InstallOptions {
    pub autostart: bool,
    /// Install folder; `None` keeps the current one (or the default).
    pub dir: Option<PathBuf>,
}

/// Installs (or updates) from the folder this program runs from, or from
/// the programs inside it (single-file installer).
pub fn install(opts: &InstallOptions) -> Result<String, String> {
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let src = me.parent().ok_or("cannot find the setup folder")?.to_path_buf();
    if !is_bundle() {
        for p in PROGRAMS {
            if !src.join(p).exists() {
                return Err(format!("{p} is missing next to the setup program. Unzip the whole download first."));
            }
        }
    }
    let old = bin_dir();
    let bin = opts.dir.clone().unwrap_or_else(|| old.clone());
    check_dir(&bin)?;
    let was_running = !test_mode() && hyprwin_ipc::request("status").is_ok();
    stop_daemon();
    if is_bundle() {
        for (p, bytes) in BUNDLE {
            write_retry(bytes, &bin.join(p)).map_err(|e| format!("cannot install {p}: {e}"))?;
        }
    } else if !same_dir(&src, &bin) {
        for p in PROGRAMS {
            copy_retry(&src.join(p), &bin.join(p)).map_err(|e| format!("cannot copy {p}: {e}"))?;
        }
        copy_retry(&me, &bin.join("hyprwin-settings.exe")).map_err(|e| format!("cannot copy the settings app: {e}"))?;
    }
    // Moved to a new folder: the old copy goes (only hyprwin's own files).
    if !same_dir(&old, &bin) && old.join("hyprwin.exe").exists() {
        remove_programs(&old, Some(&me));
    }
    let daemon = bin.join("hyprwin.exe");
    let settings = bin.join("hyprwin-settings.exe");

    let menu = start_menu_dir();
    let _ = std::fs::create_dir_all(&menu);
    let ok1 = shortcut(&menu.join(format!("{}.lnk", name())), &daemon, "", "Start hyprwin tiling");
    let ok2 = shortcut(&menu.join(format!("{} Settings.lnk", name())), &settings, "", "hyprwin settings, themes and app groups");

    let key = uninstall_key();
    let size_kb: u64 = PROGRAMS.iter().chain(std::iter::once(&"hyprwin-settings.exe")).filter_map(|p| std::fs::metadata(bin.join(p)).ok()).map(|m| m.len() / 1024).sum();
    let uninstall_cmd = format!("\"{}\" --uninstall", settings.display());
    let quiet_cmd = format!("\"{}\" --uninstall --quiet", settings.display());
    let values: [(&str, &str, String); 9] = [
        ("DisplayName", "REG_SZ", "hyprwin".into()),
        ("DisplayVersion", "REG_SZ", env!("CARGO_PKG_VERSION").into()),
        ("Publisher", "REG_SZ", "hyprwin".into()),
        ("DisplayIcon", "REG_SZ", format!("{},0", settings.display())),
        ("InstallLocation", "REG_SZ", bin.display().to_string()),
        ("UninstallString", "REG_SZ", uninstall_cmd),
        ("QuietUninstallString", "REG_SZ", quiet_cmd),
        ("NoModify", "REG_DWORD", "1".into()),
        ("EstimatedSize", "REG_DWORD", size_kb.to_string()),
    ];
    let mut ok3 = true;
    for (n, t, v) in &values {
        ok3 &= reg(&["add", &key, "/v", n, "/t", t, "/d", v, "/f"]);
    }
    ok3 &= reg(&["add", &key, "/v", "NoRepair", "/t", "REG_DWORD", "/d", "1", "/f"]);

    if opts.autostart {
        reg(&["add", RUN_KEY, "/v", name(), "/t", "REG_SZ", "/d", &format!("\"{}\"", daemon.display()), "/f"]);
    } else {
        reg(&["delete", RUN_KEY, "/v", name(), "/f"]);
    }
    if was_running {
        // The old daemon may still be releasing its single-instance lock;
        // a start that loses that race exits at once, so confirm and retry.
        for _ in 0..4 {
            let _ = std::process::Command::new(&daemon).spawn();
            let t0 = Instant::now();
            while t0.elapsed() < Duration::from_millis(2500) && hyprwin_ipc::request("status").is_err() {
                std::thread::sleep(Duration::from_millis(150));
            }
            if hyprwin_ipc::request("status").is_ok() {
                break;
            }
        }
    }
    let mut notes = Vec::new();
    if !(ok1 && ok2) {
        notes.push("Start-menu shortcuts could not be created");
    }
    if !ok3 {
        notes.push("the Installed-apps entry could not be written");
    }
    Ok(if notes.is_empty() { format!("Installed to {}", bin.display()) } else { format!("Installed to {} ({})", bin.display(), notes.join("; ")) })
}

/// Removes hyprwin. With `purge`, also the config, themes, groups, saved
/// layout and log.
pub fn uninstall(purge: bool) -> Result<String, String> {
    let bin = bin_dir(); // before its Installed-apps entry (which records it) goes
    stop_daemon();
    reg(&["delete", RUN_KEY, "/v", name(), "/f"]);
    reg(&["delete", &uninstall_key(), "/f"]);
    let menu = start_menu_dir();
    let _ = std::fs::remove_file(menu.join(format!("{}.lnk", name())));
    let _ = std::fs::remove_file(menu.join(format!("{} Settings.lnk", name())));
    let me = std::env::current_exe().ok();
    // The running program can't delete itself; the command below does once we exit.
    remove_programs(&bin, me.as_deref());
    if purge && !test_mode() {
        let _ = std::fs::remove_dir_all(hyprwin_config::paths::app_dir());
        let state = local_appdata().join("hyprwin");
        if let Ok(rd) = std::fs::read_dir(&state) {
            for e in rd.flatten() {
                let p = e.path();
                if e.file_name() != "bin" && !same_dir(&p, &bin) && !bin.starts_with(&p) {
                    let _ = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
                }
            }
        }
    }
    // Once we exit: delete this program if it runs from the install folder,
    // then the folder itself if (and only if) nothing else is in it.
    let mut cmd = format!("ping 127.0.0.1 -n 3 > nul & del /f /q \"{}\" & rmdir \"{}\"", bin.join("hyprwin-settings.exe").display(), bin.display());
    if purge && !test_mode() {
        cmd.push_str(&format!(" & rmdir \"{}\"", local_appdata().join("hyprwin").display()));
    }
    let _ = std::process::Command::new("cmd.exe").raw_arg(format!("/c {cmd}")).creation_flags(NO_WINDOW).spawn();
    Ok(if purge { "hyprwin and all its settings were removed".into() } else { "hyprwin was removed. Your config, themes and groups were kept".into() })
}

pub fn start_daemon_installed() -> Result<(), String> {
    std::process::Command::new(bin_dir().join("hyprwin.exe")).spawn().map(|_| ()).map_err(|e| e.to_string())
}
