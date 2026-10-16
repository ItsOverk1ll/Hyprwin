//! The install / uninstall window (`hyprwin-setup.exe`, or `--uninstall`).

use eframe::egui::{self, Color32, CornerRadius, RichText, Vec2};

use crate::setup;

pub const TITLE: &str = "hyprwin Setup";

#[derive(PartialEq)]
enum Page {
    Welcome,
    Done(Result<String, String>),
    ConfirmUninstall,
    Uninstalled(Result<String, String>),
}

pub struct SetupApp {
    page: Page,
    autostart: bool,
    start_now: bool,
    purge: bool,
}

impl SetupApp {
    pub fn new(cc: &eframe::CreationContext<'_>, uninstall: bool) -> Self {
        let mut v = egui::Visuals::dark();
        let accent = Color32::from_rgb(51, 204, 255);
        v.selection.bg_fill = accent.gamma_multiply(0.55);
        v.hyperlink_color = accent;
        cc.egui_ctx.set_visuals(v);
        cc.egui_ctx.all_styles_mut(|s| {
            s.spacing.item_spacing = Vec2::new(10.0, 9.0);
            s.spacing.button_padding = Vec2::new(14.0, 7.0);
        });
        SetupApp { page: if uninstall { Page::ConfirmUninstall } else { Page::Welcome }, autostart: true, start_now: true, purge: false }
    }
}

fn card(ui: &mut egui::Ui, body: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::group(ui.style()).corner_radius(CornerRadius::same(8)).inner_margin(14.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        body(ui);
    });
}

fn big_button(ui: &mut egui::Ui, text: &str) -> bool {
    ui.add(egui::Button::new(RichText::new(text).strong().size(16.0)).min_size(Vec2::new(180.0, 38.0))).clicked()
}

impl eframe::App for SetupApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().frame(egui::Frame::central_panel(ui.style()).inner_margin(egui::Margin::same(24))).show(ui, |ui| {
            ui.heading(RichText::new("hyprwin").strong().size(28.0));
            ui.label(RichText::new("Hyprland-style tiling for Windows 11").weak());
            ui.add_space(12.0);
            let mut next: Option<Page> = None;
            match &self.page {
                Page::Welcome => {
                    card(ui, |ui| {
                        ui.label("hyprwin arranges your windows side by side automatically, with keyboard shortcuts for everything, like the Hyprland desktop on Linux.");
                        ui.add_space(4.0);
                        ui.label(RichText::new("A few things worth knowing:").strong());
                        ui.label("•  Super (the Windows key) + Q opens a terminal, Super + C closes a window, Super + 1…9 switches workspaces.");
                        ui.label("•  Right-click the hyprwin icon in the system tray for Settings, Themes, App Groups and Pause.");
                        ui.label("•  If anything ever goes wrong, press  Ctrl + Alt + Shift + Win + Esc : every window goes back to normal and hyprwin quits.");
                    });
                    ui.add_space(6.0);
                    ui.checkbox(&mut self.autostart, "Start hyprwin when I sign in to Windows");
                    ui.checkbox(&mut self.start_now, "Start hyprwin now");
                    ui.label(RichText::new(format!("Installs for you only (no admin needed) to {}", setup::bin_dir().display())).weak().small());
                    ui.add_space(8.0);
                    let label = if setup::is_installed() { "Update hyprwin" } else { "Install hyprwin" };
                    if big_button(ui, label) {
                        let r = setup::install(&setup::InstallOptions { autostart: self.autostart });
                        if r.is_ok() && self.start_now && hyprwin_ipc::request("status").is_err() {
                            let _ = setup::start_daemon_installed();
                        }
                        next = Some(Page::Done(r));
                    }
                }
                Page::Done(r) => {
                    match r {
                        Ok(msg) => card(ui, |ui| {
                            ui.label(RichText::new("All set!").strong().size(18.0).color(Color32::from_rgb(120, 220, 150)));
                            ui.label(msg.as_str());
                            ui.label("Find \"hyprwin\" and \"hyprwin Settings\" in the Start menu. To remove it later, use Settings > Apps > Installed apps.");
                        }),
                        Err(e) => card(ui, |ui| {
                            ui.label(RichText::new("Something went wrong").strong().size(18.0).color(Color32::from_rgb(240, 110, 110)));
                            ui.label(e.as_str());
                        }),
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if r.is_ok() && big_button(ui, "Open Settings") {
                            let _ = std::process::Command::new(setup::bin_dir().join("hyprwin-settings.exe")).spawn();
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        if big_button(ui, "Close") {
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
                        if big_button(ui, "Cancel") {
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
                            ui.label(RichText::new("Something went wrong").strong().size(18.0).color(Color32::from_rgb(240, 110, 110)));
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
