use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, CornerRadius, RichText, Stroke, Vec2};
use hyprwin_config::settings::SettingsFile;
use hyprwin_config::themes::{self, ThemeInfo};
use hyprwin_config::{Config, paths};

use crate::daemon::{self, Status};

pub const TITLE: &str = "hyprwin";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    Settings,
    Themes,
    Groups,
    Keybinds,
    Feedback,
    About,
}

/// The editable subset of the config shown on the Settings tab.
#[derive(Clone, Debug, PartialEq)]
struct Form {
    master: bool,
    gaps_in: i32,
    gaps_out: i32,
    borders: bool,
    /// Thickness when borders are on (logical px).
    border_size: i32,
    /// The theme's focused border is a gradient (not editable here).
    gradient: bool,
    /// 0 = off, 1 = fast, 2 = normal, 3 = slow
    anim: u8,
    active: Color32,
    inactive: Color32,
    /// 0 = square, 1 = small, 2 = round
    corners: u8,
    follow_mouse: bool,
    force_tile: bool,
    hide_taskbar: bool,
}

fn to_color32(argb: u32) -> Color32 {
    Color32::from_rgba_unmultiplied((argb >> 16) as u8, (argb >> 8) as u8, argb as u8, (argb >> 24) as u8)
}

fn to_hypr(c: Color32) -> String {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    format!("rgba({r:02x}{g:02x}{b:02x}{a:02x})")
}

impl Form {
    fn from_config(c: &Config) -> Form {
        Form {
            master: c.wm.layout == hyprwin_layout::LayoutKind::Master,
            gaps_in: c.wm.gaps_in.top,
            gaps_out: c.wm.gaps_out.top,
            borders: c.border_size > 0,
            border_size: if c.border_size > 0 { c.border_size } else { 2 },
            gradient: c.active_gradient.colors.len() > 1,
            anim: if !c.animations_enabled {
                0
            } else if c.animation_speed >= 1.3 {
                1
            } else if c.animation_speed <= 0.8 {
                3
            } else {
                2
            },
            active: to_color32(c.col_active.0),
            inactive: to_color32(c.col_inactive.0),
            corners: match c.rounding {
                0 => 0,
                1..=5 => 1,
                _ => 2,
            },
            follow_mouse: c.follow_mouse != 0,
            force_tile: c.force_tile,
            hide_taskbar: c.hide_taskbar,
        }
    }

    /// The settings.conf keys for every field that differs from `base`.
    fn changes(&self, base: &Form) -> Vec<(&'static str, String)> {
        let mut v = Vec::new();
        if self.master != base.master {
            v.push(("general:layout", if self.master { "master" } else { "dwindle" }.to_string()));
        }
        if self.gaps_in != base.gaps_in {
            v.push(("general:gaps_in", self.gaps_in.to_string()));
        }
        if self.gaps_out != base.gaps_out {
            v.push(("general:gaps_out", self.gaps_out.to_string()));
        }
        if self.borders != base.borders || (self.borders && self.border_size != base.border_size) {
            v.push(("general:border_size", if self.borders { self.border_size.to_string() } else { "0".into() }));
        }
        if self.anim != base.anim {
            v.push(("animations:enabled", (self.anim != 0).to_string()));
            if self.anim != 0 {
                v.push(("hyprwin:animation_speed", ["1", "1.6", "1", "0.6"][self.anim as usize].to_string()));
            }
        }
        if self.active != base.active {
            v.push(("general:col.active_border", to_hypr(self.active)));
        }
        if self.inactive != base.inactive {
            v.push(("general:col.inactive_border", to_hypr(self.inactive)));
        }
        if self.corners != base.corners {
            v.push(("decoration:rounding", ["0", "4", "10"][self.corners as usize].to_string()));
        }
        if self.follow_mouse != base.follow_mouse {
            v.push(("input:follow_mouse", if self.follow_mouse { "1" } else { "0" }.to_string()));
        }
        if self.hide_taskbar != base.hide_taskbar {
            v.push(("hyprwin:hide_taskbar", self.hide_taskbar.to_string()));
        }
        if self.force_tile != base.force_tile {
            v.push(("hyprwin:force_tile", self.force_tile.to_string()));
        }
        v
    }
}

/// Keys a theme controls; cleared from settings.conf when a theme is applied
/// so the theme actually shows.
const LOOK_KEYS: &[&str] = &[
    "general:gaps_in",
    "general:gaps_out",
    "general:border_size",
    "general:col.active_border",
    "general:col.inactive_border",
    "decoration:rounding",
];

struct Group {
    name: String,
    apps: Vec<String>,
}

pub struct SettingsApp {
    tab: Tab,
    status: Status,
    status_at: Instant,
    form: Form,
    base: Form,
    themes: Vec<ThemeInfo>,
    active_theme: Option<PathBuf>,
    new_theme: String,
    groups: Vec<Group>,
    new_group: String,
    focus_new_group: bool,
    keybinds: crate::keybinds_ui::KeybindEditor,
    feedback: crate::feedback::Feedback,
    start_with_windows: bool,
    toast: Option<(String, Instant, bool)>,
    update_rx: Option<std::sync::mpsc::Receiver<crate::update::Outcome>>,
    update: Option<crate::update::Outcome>,
}

pub(crate) fn config_path() -> PathBuf {
    paths::active_config()
}

fn load_form() -> Form {
    Form::from_config(&Config::load_file(&config_path()))
}

fn load_groups() -> Vec<Group> {
    let mut v: Vec<Group> = std::fs::read_dir(paths::groups_dir())
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
                .filter_map(|e| {
                    let g: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(e.path()).ok()?).ok()?;
                    Some(Group {
                        name: g["name"].as_str()?.to_string(),
                        apps: g["apps"].as_array().into_iter().flatten().map(|a| a["class"].as_str().unwrap_or("?").to_string()).collect(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    v
}

const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

fn reg(args: &[&str]) -> bool {
    use std::os::windows::process::CommandExt;
    std::process::Command::new("reg.exe").args(args).creation_flags(0x0800_0000).output().map(|o| o.status.success()).unwrap_or(false)
}

fn starts_with_windows() -> bool {
    reg(&["query", RUN_KEY, "/v", "hyprwin"])
}

impl SettingsApp {
    pub fn new(cc: &eframe::CreationContext<'_>, tab: &str, save: bool) -> Self {
        themes::install_builtins();
        let form = load_form();
        let app = SettingsApp {
            tab: match tab {
                "themes" => Tab::Themes,
                "groups" => Tab::Groups,
                "keybinds" => Tab::Keybinds,
                "feedback" => Tab::Feedback,
                "about" => Tab::About,
                _ => Tab::Settings,
            },
            status: daemon::status(),
            status_at: Instant::now(),
            base: form.clone(),
            form,
            themes: themes::list(),
            active_theme: themes::active_theme(&std::fs::read_to_string(config_path()).unwrap_or_default()),
            new_theme: String::new(),
            groups: load_groups(),
            new_group: String::new(),
            focus_new_group: save,
            keybinds: Default::default(),
            feedback: Default::default(),
            start_with_windows: starts_with_windows(),
            toast: None,
            update_rx: None,
            update: None,
        };
        app.style(&cc.egui_ctx);
        app
    }

    fn style(&self, ctx: &egui::Context) {
        let mut v = egui::Visuals::dark();
        let accent = self.form.active;
        v.selection.bg_fill = accent.gamma_multiply(0.55);
        v.selection.stroke = Stroke::new(1.0, accent);
        v.hyperlink_color = accent;
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, accent.gamma_multiply(0.8));
        v.window_corner_radius = CornerRadius::same(10);
        ctx.set_visuals(v);
        ctx.all_styles_mut(|s| {
            s.spacing.item_spacing = Vec2::new(10.0, 8.0);
            s.spacing.button_padding = Vec2::new(12.0, 6.0);
        });
    }

    fn notify(&mut self, msg: impl Into<String>, error: bool) {
        self.toast = Some((msg.into(), Instant::now(), error));
    }

    fn refresh_status(&mut self) {
        if self.status_at.elapsed() > Duration::from_secs(2) {
            self.status = daemon::status();
            self.status_at = Instant::now();
        }
    }

    // ------------------------------------------------------------------ chrome

    /// Headless: `hyprwin-settings --apply-theme <name|none>` (for scripts and sharing).
    pub fn apply_theme_cli(name: &str) -> Result<String, String> {
        themes::install_builtins();
        let theme = if name.eq_ignore_ascii_case("none") {
            None
        } else {
            let wanted = name.to_lowercase();
            let t = themes::list()
                .into_iter()
                .find(|t| t.name.to_lowercase() == wanted || t.path.file_stem().is_some_and(|s| s.to_string_lossy().to_lowercase() == wanted))
                .ok_or_else(|| format!("no theme called '{name}'"))?;
            Some(t.path)
        };
        let cfg = config_path();
        let mut s = SettingsFile::load();
        for k in LOOK_KEYS {
            s.unset(k);
        }
        themes::ensure_managed_block(&cfg).and_then(|_| s.save()).and_then(|_| themes::apply(&cfg, theme.as_deref())).map_err(|e| e.to_string())?;
        daemon::reload();
        Ok(format!("applied {}", theme.map(|p| p.display().to_string()).unwrap_or_else(|| "no theme".into())))
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading(RichText::new("hyprwin").strong());
            ui.add_space(12.0);
            let (dot, text) = match self.status {
                Status::Running => (Color32::from_rgb(80, 220, 120), "Tiling is on"),
                Status::Paused => (Color32::from_rgb(240, 190, 60), "Paused: windows are behaving normally"),
                Status::NotRunning => (Color32::from_rgb(230, 80, 80), "hyprwin isn't running"),
            };
            let (r, _) = ui.allocate_exact_size(Vec2::splat(12.0), egui::Sense::hover());
            ui.painter().circle_filled(r.center(), 5.0, dot);
            ui.label(text);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| match self.status {
                Status::Running => {
                    if ui.button("Pause tiling").clicked() {
                        let _ = daemon::request("pause");
                        self.status_at = Instant::now() - Duration::from_secs(5);
                    }
                }
                Status::Paused => {
                    if ui.button("Resume tiling").clicked() {
                        let _ = daemon::request("resume");
                        self.status_at = Instant::now() - Duration::from_secs(5);
                    }
                }
                Status::NotRunning => {
                    if ui.button("Start hyprwin").clicked() {
                        match daemon::start_daemon() {
                            Ok(()) => self.notify("Starting hyprwin…", false),
                            Err(e) => self.notify(e, true),
                        }
                        self.status_at = Instant::now() - Duration::from_secs(1);
                    }
                }
            });
        });
    }

    fn nav(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        for (tab, label) in [
            (Tab::Settings, "⚙  Settings"),
            (Tab::Themes, "🎨  Themes"),
            (Tab::Groups, "🗐  App Groups"),
            (Tab::Keybinds, "⌨  Keybinds"),
            (Tab::Feedback, "💬  Feedback"),
            (Tab::About, "ℹ  About"),
        ] {
            let selected = self.tab == tab;
            let b = egui::Button::selectable(selected, RichText::new(label).size(15.0)).min_size(Vec2::new(170.0, 34.0));
            if ui.add(b).clicked() {
                self.tab = tab;
            }
        }
    }

    // ------------------------------------------------------------------ settings tab

    fn settings_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Settings");
        ui.label(RichText::new("Changes are saved to settings.conf and applied instantly. Your hyprland.conf is never rewritten.").weak());
        ui.add_space(8.0);

        section(ui, "Layout", |ui| {
            ui.horizontal(|ui| {
                ui.radio_value(&mut self.form.master, false, "Dwindle (split in halves, like Hyprland's default)");
            });
            ui.horizontal(|ui| {
                ui.radio_value(&mut self.form.master, true, "Master (one big window plus a stack)");
            });
            ui.checkbox(&mut self.form.force_tile, "Tile everything (only real dialogs float)");
        });

        section(ui, "Spacing", |ui| {
            ui.add(egui::Slider::new(&mut self.form.gaps_in, 0..=30).text("gap between windows (px)"));
            ui.add(egui::Slider::new(&mut self.form.gaps_out, 0..=60).text("gap around the screen edge (px)"));
        });

        section(ui, "Look", |ui| {
            ui.checkbox(&mut self.form.borders, "Coloured window borders");
            ui.add_enabled_ui(self.form.borders, |ui| {
                ui.add(egui::Slider::new(&mut self.form.border_size, 1..=8).text("thickness (px)"));
                if self.form.gradient && self.form.active == self.base.active {
                    ui.label(RichText::new("Your theme gives the focused window a gradient border. Picking a colour below replaces it.").weak().small());
                }
                ui.horizontal(|ui| {
                    ui.label("Focused window");
                    ui.color_edit_button_srgba(&mut self.form.active);
                    ui.add_space(16.0);
                    ui.label("Other windows");
                    ui.color_edit_button_srgba(&mut self.form.inactive);
                });
            });
            ui.horizontal(|ui| {
                ui.label("Corners");
                ui.selectable_value(&mut self.form.corners, 0, "Square");
                ui.selectable_value(&mut self.form.corners, 1, "Slightly round");
                ui.selectable_value(&mut self.form.corners, 2, "Round");
            });
        });

        section(ui, "Animations", |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.form.anim, 0, "Off");
                ui.selectable_value(&mut self.form.anim, 1, "Fast");
                ui.selectable_value(&mut self.form.anim, 2, "Normal");
                ui.selectable_value(&mut self.form.anim, 3, "Slow");
            });
            ui.label(RichText::new("Windows glide when they move, pop in when they open, and fade between workspaces.").weak());
        });

        section(ui, "Behaviour", |ui| {
            ui.checkbox(&mut self.form.follow_mouse, "Focus follows the mouse (hovering a window focuses it)");
            ui.checkbox(&mut self.form.hide_taskbar, "Hide the Windows taskbar (it shows while the mouse touches its screen edge)");
            let mut sww = self.start_with_windows;
            if ui.checkbox(&mut sww, "Start hyprwin when I sign in to Windows").changed() {
                let ok = match (sww, daemon::daemon_exe()) {
                    (true, Some(exe)) => reg(&["add", RUN_KEY, "/v", "hyprwin", "/t", "REG_SZ", "/d", &format!("\"{}\"", exe.display()), "/f"]),
                    (false, _) => reg(&["delete", RUN_KEY, "/v", "hyprwin", "/f"]),
                    (true, None) => false,
                };
                if ok {
                    self.start_with_windows = sww;
                    self.notify(if sww { "hyprwin will start when you sign in" } else { "hyprwin won't start automatically" }, false);
                } else {
                    self.notify("Couldn't change the startup setting", true);
                }
            }
        });

        ui.add_space(6.0);
        let changes = self.form.changes(&self.base);
        ui.horizontal(|ui| {
            if ui.add_enabled(!changes.is_empty(), egui::Button::new(RichText::new("Apply").strong())).clicked() {
                self.apply_settings(&changes, ui.ctx());
            }
            if ui.add_enabled(!changes.is_empty(), egui::Button::new("Undo changes")).clicked() {
                self.form = self.base.clone();
            }
            if ui.button("Reset all to config/theme").on_hover_text("Clears everything this window has set").clicked() {
                if SettingsFile::default().save().is_ok() {
                    daemon::reload();
                    self.form = load_form();
                    self.base = self.form.clone();
                    self.style(ui.ctx());
                    self.notify("Settings reset", false);
                }
            }
        });
    }

    fn apply_settings(&mut self, changes: &[(&'static str, String)], ctx: &egui::Context) {
        let mut s = SettingsFile::load();
        for (k, v) in changes {
            s.set(k, v.clone());
        }
        let res = themes::ensure_managed_block(&config_path()).and_then(|_| s.save());
        match res {
            Ok(()) => {
                daemon::reload();
                self.base = self.form.clone();
                self.style(ctx);
                self.notify("Saved and applied", false);
            }
            Err(e) => self.notify(format!("Couldn't save: {e}"), true),
        }
    }

    // ------------------------------------------------------------------ themes tab

    fn themes_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Themes");
        ui.label(RichText::new("A theme sets colours, borders, corners and gaps. Themes are plain .conf files: share them, or make your own.").weak());
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("Open themes folder").clicked() {
                let _ = std::process::Command::new("explorer.exe").arg(paths::themes_dir()).spawn();
            }
            if ui.button("Refresh").clicked() {
                self.themes = themes::list();
            }
            let none_active = self.active_theme.is_none();
            if ui.add_enabled(!none_active, egui::Button::new("Use no theme")).clicked() {
                self.apply_theme(None, ui.ctx());
            }
        });
        ui.add_space(8.0);

        let width = ui.available_width();
        let cols = ((width / 290.0).floor() as usize).max(1);
        let themes = self.themes.clone();
        let mut apply: Option<PathBuf> = None;
        egui::Grid::new("themes").num_columns(cols).spacing(Vec2::new(14.0, 14.0)).show(ui, |ui| {
            for (i, t) in themes.iter().enumerate() {
                let active = self.active_theme.as_ref() == Some(&t.path);
                if theme_card(ui, t, active) {
                    apply = Some(t.path.clone());
                }
                if (i + 1) % cols == 0 {
                    ui.end_row();
                }
            }
        });
        if let Some(p) = apply {
            self.apply_theme(Some(p), ui.ctx());
        }

        ui.add_space(14.0);
        section(ui, "Make a theme from your current look", |ui| {
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.new_theme).hint_text("Theme name").desired_width(220.0));
                if ui.add_enabled(!self.new_theme.trim().is_empty(), egui::Button::new("Save as theme")).clicked() {
                    self.save_theme(ui.ctx());
                }
            });
            ui.label(RichText::new("Uses the gaps, colours, borders and corners from the Settings tab.").weak());
        });
    }

    fn apply_theme(&mut self, theme: Option<PathBuf>, ctx: &egui::Context) {
        let cfg = config_path();
        let mut s = SettingsFile::load();
        for k in LOOK_KEYS {
            s.unset(k);
        }
        let res = themes::ensure_managed_block(&cfg).and_then(|_| s.save()).and_then(|_| themes::apply(&cfg, theme.as_deref()));
        match res {
            Ok(()) => {
                daemon::reload();
                self.active_theme = theme;
                self.form = load_form();
                self.base = self.form.clone();
                self.style(ctx);
                self.notify("Theme applied", false);
            }
            Err(e) => self.notify(format!("Couldn't apply theme: {e}"), true),
        }
    }

    fn save_theme(&mut self, ctx: &egui::Context) {
        let name = self.new_theme.trim().to_string();
        let f = &self.form;
        let text = format!(
            "# name: {name}\n# author: {}\n# description: Made in hyprwin Settings.\ngeneral {{\n    gaps_in = {}\n    gaps_out = {}\n    border_size = {}\n    col.active_border = {}\n    col.inactive_border = {}\n}}\ndecoration {{\n    rounding = {}\n}}\n",
            std::env::var("USERNAME").unwrap_or_default(),
            f.gaps_in,
            f.gaps_out,
            if f.borders { 2 } else { 0 },
            to_hypr(f.active),
            to_hypr(f.inactive),
            ["0", "4", "10"][f.corners as usize]
        );
        let path = paths::themes_dir().join(format!("{}.conf", paths::file_stem(&name).to_lowercase().replace(' ', "-")));
        match std::fs::create_dir_all(paths::themes_dir()).and_then(|_| std::fs::write(&path, text)) {
            Ok(()) => {
                self.themes = themes::list();
                self.new_theme.clear();
                self.apply_theme(Some(path), ctx);
                self.notify(format!("Saved theme '{name}'"), false);
            }
            Err(e) => self.notify(format!("Couldn't save theme: {e}"), true),
        }
    }

    // ------------------------------------------------------------------ groups tab

    fn groups_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("App Groups");
        ui.label(RichText::new("Save the apps on your current workspace and how they're arranged. Open the group any time later: hyprwin launches anything that isn't running and arranges everything as saved.").weak());
        ui.add_space(8.0);

        section(ui, "Save the current workspace as a group", |ui| {
            ui.horizontal(|ui| {
                let edit = ui.add(egui::TextEdit::singleline(&mut self.new_group).hint_text("e.g. Coding: Terminal + VS Code + Claude").desired_width(340.0));
                if self.focus_new_group {
                    edit.request_focus();
                    self.focus_new_group = false;
                }
                let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let can = !self.new_group.trim().is_empty() && self.status == Status::Running;
                if (ui.add_enabled(can, egui::Button::new("Save group")).clicked() || (enter && can)) && can {
                    match daemon::request(&format!("groupsave {}", self.new_group.trim())) {
                        Ok(r) if !r.starts_with("error") => {
                            self.notify(format!("Saved: {r}"), false);
                            self.new_group.clear();
                            self.groups = load_groups();
                        }
                        Ok(r) => self.notify(r.trim_start_matches("error: ").to_string(), true),
                        Err(e) => self.notify(e, true),
                    }
                }
            });
            if self.status != Status::Running {
                ui.label(RichText::new("Tiling must be on to save a group.").weak());
            }
        });

        ui.add_space(6.0);
        if self.groups.is_empty() {
            ui.label(RichText::new("No groups yet.").italics());
        }
        let mut delete: Option<String> = None;
        for g in &self.groups {
            egui::Frame::group(ui.style()).corner_radius(CornerRadius::same(8)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&g.name).strong().size(15.0));
                        ui.label(RichText::new(g.apps.join("  ·  ")).weak());
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Delete").clicked() {
                            delete = Some(g.name.clone());
                        }
                        if ui.add_enabled(self.status == Status::Running, egui::Button::new(RichText::new("Open").strong())).clicked() {
                            match daemon::request(&format!("groupopen {}", g.name)) {
                                Ok(r) if !r.starts_with("error") => self.toast = Some((format!("Opening '{}'…", g.name), Instant::now(), false)),
                                Ok(r) => self.toast = Some((r, Instant::now(), true)),
                                Err(e) => self.toast = Some((e, Instant::now(), true)),
                            }
                        }
                    });
                });
            });
        }
        if let Some(name) = delete {
            let path = paths::groups_dir().join(format!("{}.json", paths::file_stem(&name)));
            match std::fs::remove_file(path) {
                Ok(()) => self.notify(format!("Deleted '{name}'"), false),
                Err(e) => self.notify(format!("Couldn't delete: {e}"), true),
            }
            self.groups = load_groups();
        }
    }

    // ------------------------------------------------------------------ keybinds / about

    fn keybinds_tab(&mut self, ui: &mut egui::Ui) {
        let mut toast: Option<(String, bool)> = None;
        self.keybinds.show(ui, &mut |m, e| toast = Some((m, e)));
        if let Some((m, e)) = toast {
            self.notify(m, e);
        }
    }

    fn about_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("About hyprwin");
        ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
        if crate::update::is_prerelease() {
            ui.colored_label(egui::Color32::from_rgb(255, 190, 90), "Test version (beta): not the stable release. Please report anything odd in the Feedback tab.");
        }
        ui.label("Hyprland-style tiling for Windows 11.");
        ui.add_space(10.0);
        section(ui, "Updates", |ui| {
            if let Some(rx) = &self.update_rx {
                if let Ok(o) = rx.try_recv() {
                    self.update = Some(o);
                    self.update_rx = None;
                }
            }
            use crate::update::Outcome;
            ui.horizontal(|ui| {
                let checking = self.update_rx.is_some();
                if ui.add_enabled(!checking, egui::Button::new(if checking { "Checking…" } else { "Check for updates" })).clicked() {
                    self.update = None;
                    self.update_rx = Some(crate::update::start());
                }
                match &self.update {
                    Some(Outcome::UpToDate(v)) => {
                        ui.label(RichText::new(format!("You have the latest version ({v}).")).color(Color32::from_rgb(120, 220, 150)));
                    }
                    Some(Outcome::Available { version, url }) => {
                        ui.label(RichText::new(format!("hyprwin {version} is available.")).strong());
                        if ui.button("Open download page").clicked() {
                            crate::update::open(url);
                        }
                    }
                    Some(Outcome::Failed(why)) => {
                        ui.label(RichText::new(format!("Couldn't check: {why}.")).weak());
                        if ui.button("Open releases page").clicked() {
                            crate::update::open(crate::update::RELEASES_PAGE);
                        }
                    }
                    None => {}
                }
            });
            ui.label(RichText::new("To update, download the new zip and run hyprwin-setup.exe; your settings are kept.").weak().small());
        });
        section(ui, "If something goes wrong", |ui| {
            ui.label(RichText::new("Ctrl + Alt + Shift + Win + Esc").monospace().strong());
            ui.label("Restores every window and quits hyprwin immediately. Always works.");
        });
        section(ui, "Files", |ui| {
            ui.horizontal(|ui| {
                ui.label(format!("Config: {}", config_path().display()));
                if ui.small_button("Open").clicked() {
                    let _ = std::process::Command::new("notepad.exe").arg(config_path()).spawn();
                }
            });
            let log = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_default().join("hyprwin").join("hyprwin.log");
            ui.horizontal(|ui| {
                ui.label(format!("Log: {}", log.display()));
                if ui.small_button("Open").clicked() {
                    let _ = std::process::Command::new("notepad.exe").arg(&log).spawn();
                }
            });
            ui.horizontal(|ui| {
                ui.label(format!("Themes, groups, settings: {}", paths::app_dir().display()));
                if ui.small_button("Open").clicked() {
                    let _ = std::process::Command::new("explorer.exe").arg(paths::app_dir()).spawn();
                }
            });
        });
    }
}

fn section(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::group(ui.style()).corner_radius(CornerRadius::same(8)).inner_margin(12.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new(title).strong().size(15.0));
        ui.add_space(4.0);
        body(ui);
    });
    ui.add_space(8.0);
}

/// Outlines `r` with a gradient (as hyprwin draws it: 0deg = left to right),
/// as short segments coloured by their position along the gradient axis.
fn gradient_outline(p: &egui::Painter, r: egui::Rect, g: &hyprwin_config::Gradient) {
    let a = g.angle_deg.to_radians();
    let (dx, dy) = (a.cos() as f32, -(a.sin() as f32));
    let c = r.center();
    let half = (r.width() * dx.abs() + r.height() * dy.abs()) / 2.0;
    let color_at = |pt: egui::Pos2| {
        let t = if half > 0.0 { ((pt.x - c.x) * dx + (pt.y - c.y) * dy) / (2.0 * half) + 0.5 } else { 0.0 };
        let pos = t.clamp(0.0, 1.0) * (g.colors.len() - 1) as f32;
        let i = (pos.floor() as usize).min(g.colors.len() - 2);
        let f = pos - i as f32;
        let (x, y) = (to_color32(g.colors[i].0), to_color32(g.colors[i + 1].0));
        Color32::from_rgb(
            (x.r() as f32 + (y.r() as f32 - x.r() as f32) * f) as u8,
            (x.g() as f32 + (y.g() as f32 - x.g() as f32) * f) as u8,
            (x.b() as f32 + (y.b() as f32 - x.b() as f32) * f) as u8,
        )
    };
    let corners = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
    for w in corners.windows(2) {
        let n = 24;
        for k in 0..n {
            let a = w[0] + (w[1] - w[0]) * (k as f32 / n as f32);
            let b = w[0] + (w[1] - w[0]) * ((k + 1) as f32 / n as f32);
            p.line_segment([a, b], Stroke::new(2.0, color_at(a.lerp(b, 0.5))));
        }
    }
}

/// A theme preview card. Returns true when "Apply" is clicked.
fn theme_card(ui: &mut egui::Ui, t: &ThemeInfo, active: bool) -> bool {
    let mut clicked = false;
    let accent = t.active.map(to_color32).unwrap_or(Color32::from_rgb(51, 204, 255));
    let stroke = if active { Stroke::new(2.0, accent) } else { Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color) };
    egui::Frame::group(ui.style()).stroke(stroke).corner_radius(CornerRadius::same(10)).inner_margin(10.0).show(ui, |ui| {
        ui.vertical(|ui| {
        ui.set_width(260.0);
        // Mini desktop preview: three tiles with the theme's gaps/colours/corners.
        let (rect, _) = ui.allocate_exact_size(Vec2::new(260.0, 120.0), egui::Sense::hover());
        let p = ui.painter();
        p.rect_filled(rect, CornerRadius::same(6), Color32::from_rgb(24, 26, 34));
        let num = |s: &Option<String>, d: f32| s.as_deref().and_then(|v| v.split([',', ' ']).next()?.trim().parse::<f32>().ok()).unwrap_or(d);
        let go = num(&t.gaps_out, 20.0) * 0.35;
        let gi = num(&t.gaps_in, 5.0) * 0.35;
        let r = num(&t.rounding, 10.0).clamp(0.0, 10.0) * 0.5;
        let inner = rect.shrink(go.max(2.0));
        let left = egui::Rect::from_min_max(inner.min, egui::pos2(inner.center().x - gi, inner.max.y));
        let rt = egui::Rect::from_min_max(egui::pos2(inner.center().x + gi, inner.min.y), egui::pos2(inner.max.x, inner.center().y - gi));
        let rb = egui::Rect::from_min_max(egui::pos2(inner.center().x + gi, inner.center().y + gi), inner.max);
        let inactive = t.inactive.map(to_color32).unwrap_or(Color32::from_gray(90));
        for (i, tile) in [left, rt, rb].into_iter().enumerate() {
            p.rect_filled(tile, CornerRadius::same(r as u8), Color32::from_rgb(40, 43, 54));
            match t.active_gradient.as_ref().filter(|g| i == 0 && g.colors.len() > 1) {
                Some(g) => gradient_outline(p, tile.shrink(1.0), g),
                None => {
                    let c = if i == 0 { accent } else { inactive };
                    p.rect_stroke(tile, CornerRadius::same(r as u8), Stroke::new(2.0, c), egui::StrokeKind::Inside);
                }
            }
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(&t.name).strong().size(15.0));
            if active {
                ui.label(RichText::new("✔ active").color(accent));
            }
        });
        if !t.author.is_empty() {
            ui.label(RichText::new(format!("by {}", t.author)).weak().small());
        }
        ui.add(egui::Label::new(RichText::new(&t.description).weak()).wrap());
        if ui.add_enabled(!active, egui::Button::new("Apply")).clicked() {
            clicked = true;
        }
        });
    });
    clicked
}

impl eframe::App for SettingsApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.refresh_status();
        let bg = ui.visuals().panel_fill;
        egui::Panel::top("top").frame(egui::Frame::new().fill(bg).inner_margin(egui::Margin::symmetric(14, 10))).show(ui, |ui| self.top_bar(ui));
        egui::Panel::left("nav").resizable(false).exact_size(190.0).frame(egui::Frame::new().fill(bg).inner_margin(egui::Margin::same(8))).show(ui, |ui| self.nav(ui));
        if let Some((msg, at, err)) = &self.toast {
            if at.elapsed() < Duration::from_secs(4) {
                let (msg, err) = (msg.clone(), *err);
                egui::Panel::bottom("toast").frame(egui::Frame::new().fill(bg).inner_margin(egui::Margin::symmetric(14, 8))).show(ui, |ui| {
                    ui.label(RichText::new(msg).color(if err { Color32::from_rgb(240, 110, 110) } else { Color32::from_rgb(120, 220, 150) }));
                });
            } else {
                self.toast = None;
            }
        }
        egui::CentralPanel::default().frame(egui::Frame::central_panel(ui.style()).inner_margin(egui::Margin::same(16))).show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                match self.tab {
                    Tab::Settings => self.settings_tab(ui),
                    Tab::Themes => self.themes_tab(ui),
                    Tab::Groups => self.groups_tab(ui),
                    Tab::Keybinds => self.keybinds_tab(ui),
                    Tab::Feedback => self.feedback.show(ui),
                    Tab::About => self.about_tab(ui),
                }
            });
        });
        // Poll the daemon's status gently while open.
        ui.ctx().request_repaint_after(Duration::from_secs(2));
    }
}
