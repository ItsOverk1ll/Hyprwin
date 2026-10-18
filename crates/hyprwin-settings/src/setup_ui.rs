//! The install / uninstall window (`hyprwin-setup.exe`, or `--uninstall`).
//!
//! Install is a short wizard: what hyprwin is, how to use it, where to put
//! it, then a "you're done, here's what to try" page. The install itself
//! runs on a worker thread so the window never freezes.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

use eframe::egui::{self, Color32, CornerRadius, RichText, Vec2};

use crate::{setup, update};

pub const TITLE: &str = "hyprwin Setup";

const ACCENT: Color32 = Color32::from_rgb(51, 204, 255);
const GOOD: Color32 = Color32::from_rgb(120, 220, 150);
const BAD: Color32 = Color32::from_rgb(240, 110, 110);
const WARN: Color32 = Color32::from_rgb(255, 190, 90);

enum Page {
    Welcome,
    Guide,
    Location,
    Installing(Receiver<Result<String, String>>),
    Done(Result<String, String>),
    ConfirmUninstall,
    Uninstalled(Result<String, String>),
}

pub struct SetupApp {
    page: Page,
    autostart: bool,
    start_now: bool,
    purge: bool,
    dir: String,
    dir_error: Option<String>,
    installed_in: Option<PathBuf>,
    logo: Option<egui::TextureHandle>,
}

impl SetupApp {
    pub fn new(cc: &eframe::CreationContext<'_>, uninstall: bool) -> Self {
        let mut v = egui::Visuals::dark();
        v.selection.bg_fill = ACCENT.gamma_multiply(0.55);
        v.hyperlink_color = ACCENT;
        cc.egui_ctx.set_visuals(v);
        cc.egui_ctx.all_styles_mut(|s| {
            s.spacing.item_spacing = Vec2::new(10.0, 8.0);
            s.spacing.button_padding = Vec2::new(14.0, 7.0);
        });
        let logo = image::load_from_memory_with_format(crate::TASKBAR_ICO, image::ImageFormat::Ico).ok().map(|img| {
            let img = img.into_rgba8();
            let size = [img.width() as usize, img.height() as usize];
            // Pixel art: keep the edges crisp when scaled.
            cc.egui_ctx.load_texture("logo", egui::ColorImage::from_rgba_unmultiplied(size, &img), egui::TextureOptions::NEAREST)
        });
        let installed_in = setup::is_installed().then(setup::bin_dir);
        let dir = installed_in.clone().unwrap_or_else(setup::default_dir).display().to_string();
        SetupApp {
            page: if uninstall { Page::ConfirmUninstall } else { Page::Welcome },
            autostart: true,
            start_now: true,
            purge: false,
            dir,
            dir_error: None,
            installed_in,
            logo,
        }
    }

    fn header(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if let Some(t) = &self.logo {
                ui.add(egui::Image::new(t).fit_to_exact_size(Vec2::splat(64.0)));
                ui.add_space(6.0);
            }
            ui.vertical(|ui| {
                ui.add_space(4.0);
                ui.label(RichText::new("hyprwin").strong().size(28.0));
                ui.label(RichText::new(format!("Hyprland-style tiling for Windows 11  ·  version {}", update::VERSION)).weak());
            });
        });
        if update::is_prerelease() {
            ui.add_space(6.0);
            egui::Frame::new().fill(WARN.gamma_multiply(0.18)).stroke(egui::Stroke::new(1.0, WARN)).corner_radius(CornerRadius::same(6)).inner_margin(10.0).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new("Test version (beta)").strong().color(WARN));
                ui.label("This isn't the stable release yet. Most things should work, but some may not. If something looks wrong, press Ctrl + Alt + Shift + Win + Esc to put every window back, and let us know what happened.");
            });
        }
        ui.add_space(10.0);
    }

    fn step(ui: &mut egui::Ui, n: u8, name: &str) {
        ui.label(RichText::new(format!("Step {n} of 3  ·  {name}")).color(ACCENT).strong());
    }
}

fn card(ui: &mut egui::Ui, body: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::group(ui.style()).corner_radius(CornerRadius::same(8)).inner_margin(14.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        body(ui);
    });
}

fn big_button(ui: &mut egui::Ui, text: &str) -> bool {
    ui.add(egui::Button::new(RichText::new(text).strong().size(16.0)).min_size(Vec2::new(150.0, 36.0))).clicked()
}

fn small_button(ui: &mut egui::Ui, text: &str) -> bool {
    ui.add(egui::Button::new(RichText::new(text).size(15.0)).min_size(Vec2::new(110.0, 36.0))).clicked()
}

/// A row of "key → what it does".
fn keys(ui: &mut egui::Ui, rows: &[(&str, &str)]) {
    egui::Grid::new(rows[0].0).num_columns(2).spacing([18.0, 6.0]).show(ui, |ui| {
        for (k, what) in rows {
            ui.label(RichText::new(*k).monospace().strong());
            ui.label(*what);
            ui.end_row();
        }
    });
}

fn mb(bytes: u64) -> String {
    let m = bytes as f64 / (1024.0 * 1024.0);
    if m >= 1024.0 { format!("{:.1} GB", m / 1024.0) } else { format!("{m:.0} MB") }
}

/// The folder hyprwin goes in, from what the user picked: a folder not
/// named for hyprwin gets a `hyprwin` folder inside it, so the install
/// never mixes its files into a folder holding other things.
fn install_folder_for(picked: &Path) -> PathBuf {
    let named = picked.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase().contains("hyprwin"));
    if named || picked.join("hyprwin.exe").exists() { picked.to_path_buf() } else { picked.join("hyprwin") }
}

/// Windows' own "choose a folder" dialog.
fn pick_folder(start: &Path) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree};
    use windows::Win32::UI::Shell::{FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog, IShellItem, SHCreateItemFromParsingName, SIGDN_FILESYSPATH};
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    use windows::core::{PCWSTR, w};
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let dlg: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let opts = dlg.GetOptions().ok()?;
        dlg.SetOptions(opts | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM).ok()?;
        let _ = dlg.SetTitle(w!("Choose where to install hyprwin"));
        let mut p = start;
        while !p.exists() {
            match p.parent() {
                Some(q) => p = q,
                None => break,
            }
        }
        if p.exists() {
            let wide: Vec<u16> = p.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
            if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(PCWSTR(wide.as_ptr()), None) {
                let _ = dlg.SetFolder(&item);
            }
        }
        dlg.Show(Some(GetForegroundWindow())).ok()?;
        let item = dlg.GetResult().ok()?;
        let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = name.to_string().ok();
        CoTaskMemFree(Some(name.0 as *const _));
        s.map(PathBuf::from)
    }
}

impl eframe::App for SetupApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().frame(egui::Frame::central_panel(ui.style()).inner_margin(egui::Margin::same(22))).show(ui, |ui| {
            self.header(ui);
            let mut next: Option<Page> = None;
            match &self.page {
                Page::Welcome => {
                    Self::step(ui, 1, "Welcome");
                    card(ui, |ui| {
                        ui.label(RichText::new("What hyprwin does").strong().size(17.0));
                        ui.label("hyprwin arranges your open windows side by side automatically, so they never overlap and always use the whole screen. Open a new app and the others make room; close one and the rest fill the gap.");
                        ui.add_space(4.0);
                        ui.label("You also get workspaces: up to 9 separate desktops you can flip between with one key, each with its own set of windows.");
                        ui.add_space(4.0);
                        ui.label(RichText::new("Nothing is changed permanently. Pausing or closing hyprwin puts every window back to normal Windows behaviour.").weak());
                    });
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if big_button(ui, "Next  ›") {
                            next = Some(Page::Guide);
                        }
                        if small_button(ui, "Cancel") {
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });
                }
                Page::Guide => {
                    Self::step(ui, 2, "How to use it");
                    egui::ScrollArea::vertical().max_height(ui.available_height() - 60.0).show(ui, |ui| {
                        card(ui, |ui| {
                            ui.label(RichText::new("The Super key is the Windows key (⊞). Hold it and press:").strong());
                            ui.add_space(2.0);
                            keys(
                                ui,
                                &[
                                    ("Super + Q", "open a terminal"),
                                    ("Super + C", "close the window you're in"),
                                    ("Super + 1 … 9", "switch to workspace 1 … 9"),
                                    ("Super + Shift + 1 … 9", "send the window to another workspace"),
                                    ("Super + arrow keys", "move between windows"),
                                    ("Super + Shift + arrows", "swap a window with its neighbour"),
                                    ("Super + V", "let a window float freely (press again to tile it)"),
                                    ("Super + F", "full screen on / off"),
                                    ("Super + drag", "move a window with the mouse"),
                                ],
                            );
                        });
                        ui.add_space(6.0);
                        card(ui, |ui| {
                            ui.label(RichText::new("The tray icon").strong());
                            ui.label("hyprwin lives in the system tray (bottom-right, next to the clock; you may need to click the ^ arrow). Right-click it to Pause, open Settings, change the Theme, use App Groups, or Quit.");
                            ui.add_space(4.0);
                            ui.label(RichText::new("If anything goes wrong").strong());
                            ui.label("Press Ctrl + Alt + Shift + Win + Esc. Every window goes back to normal and hyprwin quits. You can start it again from the Start menu.");
                            ui.add_space(4.0);
                            ui.label(RichText::new("Every shortcut can be changed later in Settings › Keybinds.").weak());
                        });
                    });
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if big_button(ui, "Next  ›") {
                            next = Some(Page::Location);
                        }
                        if small_button(ui, "‹  Back") {
                            next = Some(Page::Welcome);
                        }
                    });
                }
                Page::Location => {
                    Self::step(ui, 3, "Where to install");
                    let target = PathBuf::from(self.dir.trim());
                    card(ui, |ui| {
                        if let Some(at) = &self.installed_in {
                            ui.label(format!("hyprwin is already installed in {}. Installing again updates it; your settings, themes and groups are kept.", at.display()));
                            ui.add_space(4.0);
                        }
                        ui.label(RichText::new("Install folder").strong());
                        ui.horizontal(|ui| {
                            let edit = egui::TextEdit::singleline(&mut self.dir).desired_width(ui.available_width() - 120.0);
                            if ui.add(edit).changed() {
                                self.dir_error = None;
                            }
                            if ui.button("Browse…").clicked()
                                && let Some(p) = pick_folder(&target)
                            {
                                self.dir = install_folder_for(&p).display().to_string();
                                self.dir_error = None;
                            }
                        });
                        let suggested = self.installed_in.clone().unwrap_or_else(setup::default_dir);
                        if target != suggested && ui.link("Use the suggested folder").clicked() {
                            self.dir = suggested.display().to_string();
                            self.dir_error = None;
                        }
                        ui.label(RichText::new("The suggested folder is in your user profile, so no admin rights are needed. If you pick another folder, hyprwin gets its own \"hyprwin\" folder inside it.").weak().size(13.0));
                        let need = setup::install_size();
                        match setup::free_space(&target) {
                            Some(free) => ui.label(RichText::new(format!("Needs about {}  ·  {} free on that drive", mb(need), mb(free))).small()),
                            None => ui.label(RichText::new(format!("Needs about {}", mb(need))).small()),
                        };
                        if let Some(e) = &self.dir_error {
                            ui.label(RichText::new(e.as_str()).color(BAD));
                        }
                    });
                    ui.add_space(6.0);
                    card(ui, |ui| {
                        ui.checkbox(&mut self.autostart, "Start hyprwin automatically when I sign in to Windows");
                        ui.checkbox(&mut self.start_now, "Start hyprwin as soon as it's installed");
                        ui.label(RichText::new("You'll also get \"hyprwin\" and \"hyprwin Settings\" in the Start menu, and it can be removed any time from Windows Settings › Apps › Installed apps.").weak().size(13.0));
                    });
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        let label = if self.installed_in.is_some() { "Update hyprwin" } else { "Install hyprwin" };
                        if big_button(ui, label) {
                            match setup::check_dir(&target) {
                                Err(e) => self.dir_error = Some(e),
                                Ok(()) => {
                                    let (tx, rx) = channel();
                                    let opts = setup::InstallOptions { autostart: self.autostart, dir: Some(target.clone()) };
                                    let start_now = self.start_now;
                                    let ctx = ui.ctx().clone();
                                    std::thread::spawn(move || {
                                        let r = setup::install(&opts);
                                        if r.is_ok() && start_now && hyprwin_ipc::request("status").is_err() {
                                            let _ = setup::start_daemon_installed();
                                        }
                                        let _ = tx.send(r);
                                        ctx.request_repaint();
                                    });
                                    next = Some(Page::Installing(rx));
                                }
                            }
                        }
                        if small_button(ui, "‹  Back") {
                            next = Some(Page::Guide);
                        }
                    });
                }
                Page::Installing(rx) => {
                    card(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(RichText::new("Installing hyprwin…").strong().size(17.0));
                        });
                        ui.label("Copying the programs, adding Start-menu shortcuts and the Installed-apps entry. This takes a few seconds.");
                    });
                    match rx.try_recv() {
                        Ok(r) => next = Some(Page::Done(r)),
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => next = Some(Page::Done(Err("the installer stopped unexpectedly".into()))),
                        Err(std::sync::mpsc::TryRecvError::Empty) => ui.ctx().request_repaint_after(std::time::Duration::from_millis(100)),
                    }
                }
                Page::Done(r) => {
                    match r {
                        Ok(msg) => {
                            card(ui, |ui| {
                                ui.label(RichText::new("All set!").strong().size(20.0).color(GOOD));
                                ui.label(msg.as_str());
                            });
                            ui.add_space(6.0);
                            card(ui, |ui| {
                                ui.label(RichText::new("Try it out").strong());
                                if self.start_now {
                                    ui.label("•  hyprwin is running now. Look for its drop icon in the system tray (bottom-right; maybe under the ^ arrow).");
                                } else {
                                    ui.label("•  Start it from the Start menu: search for \"hyprwin\".");
                                }
                                ui.label("•  Open two or three apps and watch them line up side by side.");
                                ui.label("•  Press Super + 2 to go to an empty workspace, then Super + 1 to come back.");
                                ui.label("•  Change shortcuts, colours and more in \"hyprwin Settings\" (Start menu or the tray icon).");
                                if update::is_prerelease() {
                                    ui.add_space(4.0);
                                    ui.label(RichText::new("This is a test version: if anything seems off, open hyprwin Settings › Feedback and tell us. It only takes a minute.").color(WARN));
                                }
                            });
                        }
                        Err(e) => card(ui, |ui| {
                            ui.label(RichText::new("Something went wrong").strong().size(18.0).color(BAD));
                            ui.label(e.as_str());
                            ui.label(RichText::new("Nothing on your PC was changed beyond what's listed above. You can go back and try again, or pick another folder.").weak());
                        }),
                    }
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if r.is_ok() {
                            if big_button(ui, "Open Settings") {
                                let _ = std::process::Command::new(setup::bin_dir().join("hyprwin-settings.exe")).spawn();
                                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                        } else if big_button(ui, "‹  Back") {
                            next = Some(Page::Location);
                        }
                        if small_button(ui, "Close") {
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });
                }
                Page::ConfirmUninstall => {
                    card(ui, |ui| {
                        ui.label(RichText::new("Remove hyprwin?").strong().size(18.0));
                        ui.label("hyprwin will stop, every window and the taskbar go back to normal, and its programs, shortcuts and startup entry are removed.");
                    });
                    ui.add_space(4.0);
                    ui.checkbox(&mut self.purge, "Also delete my hyprwin config, themes and app groups");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if big_button(ui, "Uninstall") {
                            next = Some(Page::Uninstalled(setup::uninstall(self.purge)));
                        }
                        if small_button(ui, "Cancel") {
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });
                }
                Page::Uninstalled(r) => {
                    card(ui, |ui| match r {
                        Ok(m) => {
                            ui.label(RichText::new("Removed").strong().size(18.0));
                            ui.label(m.as_str());
                        }
                        Err(e) => {
                            ui.label(RichText::new("Something went wrong").strong().size(18.0).color(BAD));
                            ui.label(e.as_str());
                        }
                    });
                    ui.add_space(8.0);
                    if big_button(ui, "Close") {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
            }
            if let Some(p) = next {
                self.page = p;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picked_folders_get_their_own_hyprwin_folder() {
        assert_eq!(install_folder_for(Path::new(r"D:\Apps")), PathBuf::from(r"D:\Apps\hyprwin"));
        assert_eq!(install_folder_for(Path::new(r"D:\Apps\hyprwin")), PathBuf::from(r"D:\Apps\hyprwin"));
        assert_eq!(install_folder_for(Path::new(r"D:\Tools\Hyprwin-beta")), PathBuf::from(r"D:\Tools\Hyprwin-beta"));
    }
}
