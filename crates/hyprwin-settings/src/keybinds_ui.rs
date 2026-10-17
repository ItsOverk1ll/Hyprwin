//! The Keybinds tab: view, change, add and remove keybinds without touching
//! hyprland.conf (changes go to binds.conf, see `hyprwin_config::binds_file`).

use eframe::egui::{self, Color32, RichText, Vec2};
use hyprwin_config::binds_file::{ACTIONS, ArgKind, BindLine, BindsFile, DIRECTIONS, key_choices, match_action};
use hyprwin_config::{Bind, Config, Key, Mods, themes};

use crate::daemon;

/// What a pressed combo looks like to people: "Super + Shift + Q".
pub fn pretty_combo(m: Mods, k: Key) -> String {
    let mut parts: Vec<String> = Vec::new();
    for (bit, name) in [(Mods::SUPER, "Super"), (Mods::CTRL, "Ctrl"), (Mods::ALT, "Alt"), (Mods::SHIFT, "Shift")] {
        if m.contains(bit) {
            parts.push(name.into());
        }
    }
    parts.push(k.label());
    parts.join(" + ")
}

#[derive(Clone, Debug)]
struct Draft {
    original: Option<(Mods, Key)>,
    flags: String,
    sup: bool,
    ctrl: bool,
    alt: bool,
    shift: bool,
    key: String,
    /// Index into ACTIONS, or None for a custom dispatcher.
    action: Option<usize>,
    arg: String,
    custom_disp: String,
    custom_arg: String,
    error: Option<String>,
}

impl Draft {
    fn new() -> Draft {
        Draft {
            original: None,
            flags: String::new(),
            sup: true,
            ctrl: false,
            alt: false,
            shift: false,
            key: "A".into(),
            action: Some(0),
            arg: String::new(),
            custom_disp: String::new(),
            custom_arg: String::new(),
            error: None,
        }
    }

    fn from_bind(b: &Bind) -> Draft {
        let matched = match_action(&b.dispatcher, &b.arg);
        // Flags other than the description flag are kept as they were.
        let flags: String = [(b.flags.locked, 'l'), (b.flags.release, 'r'), (b.flags.repeat, 'e'), (b.flags.mouse, 'm'), (b.flags.non_consuming, 'n'), (b.flags.transparent, 't'), (b.flags.ignore_mods, 'i')]
            .iter()
            .filter(|(on, _)| *on)
            .map(|(_, c)| *c)
            .collect();
        Draft {
            original: Some((b.mods, b.key)),
            flags,
            sup: b.mods.contains(Mods::SUPER),
            ctrl: b.mods.contains(Mods::CTRL),
            alt: b.mods.contains(Mods::ALT),
            shift: b.mods.contains(Mods::SHIFT),
            key: b.key.name(),
            action: matched.as_ref().map(|m| m.0),
            arg: matched.map(|m| m.1).unwrap_or_default(),
            custom_disp: b.dispatcher.clone(),
            custom_arg: b.arg.clone(),
            error: None,
        }
    }

    fn mods(&self) -> Mods {
        let mut m = 0;
        for (on, bit) in [(self.sup, Mods::SUPER), (self.ctrl, Mods::CTRL), (self.alt, Mods::ALT), (self.shift, Mods::SHIFT)] {
            if on {
                m |= bit.0;
            }
        }
        Mods(m)
    }

    /// The bind line this draft describes, or why it can't be saved.
    fn line(&self) -> Result<BindLine, String> {
        let key = Key::parse(&self.key).map_err(|e| format!("key: {e}"))?;
        let (dispatcher, arg) = match self.action {
            Some(i) => {
                let a = &ACTIONS[i];
                let arg = match a.arg {
                    ArgKind::Fixed(f) => f.to_string(),
                    ArgKind::Workspace | ArgKind::Direction => self.arg.clone(),
                    ArgKind::Text(_) => self.arg.trim().to_string(),
                };
                if arg.is_empty() && !matches!(a.arg, ArgKind::Fixed(_)) {
                    return Err("fill in the action's details".into());
                }
                (a.dispatcher.to_string(), arg)
            }
            None => {
                if self.custom_disp.trim().is_empty() {
                    return Err("type a dispatcher".into());
                }
                (self.custom_disp.trim().to_string(), self.custom_arg.trim().to_string())
            }
        };
        let line = BindLine { flags: self.flags.clone(), mods: self.mods(), key, dispatcher, arg };
        // Let the real config parser judge it.
        let c = Config::load_str(&line.render());
        if let Some(w) = c.warnings.first() {
            return Err(w.trim_start_matches("<string>:1: ").to_string());
        }
        if c.binds.len() != 1 {
            return Err("that keybind isn't valid".into());
        }
        Ok(line)
    }
}

/// `--bind "MODS, key, dispatcher[, arg]"` / `--unbind "MODS, key"`.
pub fn apply_cli(bind: bool, spec: &str) -> Result<(), String> {
    let mut p = spec.splitn(4, ',').map(str::trim);
    let mods = Mods::parse(p.next().unwrap_or(""))?;
    let key = Key::parse(p.next().ok_or("missing key")?)?;
    let mut f = BindsFile::load();
    if bind {
        let dispatcher = p.next().ok_or("missing dispatcher")?.to_string();
        let line = BindLine { flags: String::new(), mods, key, dispatcher, arg: p.next().unwrap_or("").to_string() };
        let c = Config::load_str(&line.render());
        if let Some(w) = c.warnings.first() {
            return Err(w.clone());
        }
        let original = Config::load_file(&crate::ui::config_path()).effective_binds().iter().any(|b| (b.mods, b.key) == (mods, key)).then_some((mods, key));
        f.set(original, line);
    } else {
        f.remove((mods, key));
    }
    themes::ensure_managed_block(&crate::ui::config_path()).map_err(|e| e.to_string())?;
    f.save().map_err(|e| e.to_string())?;
    daemon::reload();
    Ok(())
}

#[derive(Default)]
pub struct KeybindEditor {
    binds: Vec<Bind>,
    loaded: bool,
    draft: Option<Draft>,
}

impl KeybindEditor {
    fn reload(&mut self) {
        self.binds = Config::load_file(&crate::ui::config_path()).effective_binds();
        self.loaded = true;
    }

    fn save(&mut self, f: &BindsFile) -> Result<(), String> {
        themes::ensure_managed_block(&crate::ui::config_path()).map_err(|e| e.to_string())?;
        f.save().map_err(|e| e.to_string())?;
        daemon::reload();
        self.reload();
        Ok(())
    }

    /// Draws the tab. `notify(message, is_error)` shows a toast.
    pub fn show(&mut self, ui: &mut egui::Ui, notify: &mut dyn FnMut(String, bool)) {
        if !self.loaded {
            self.reload();
        }
        ui.heading("Keybinds");
        ui.label(RichText::new("Change any keybind here. Changes are saved to binds.conf and apply instantly; your hyprland.conf isn't touched.").weak());
        ui.horizontal(|ui| {
            if ui.button("Add keybind").clicked() {
                self.draft = Some(Draft::new());
            }
            if ui.button("Reset to config defaults").on_hover_text("Removes every change made here").clicked() {
                match self.save(&BindsFile::default()) {
                    Ok(()) => notify("Keybinds reset".into(), false),
                    Err(e) => notify(format!("Couldn't reset: {e}"), true),
                }
            }
            if ui.button("Edit config file").clicked() {
                let _ = std::process::Command::new("notepad.exe").arg(crate::ui::config_path()).spawn();
            }
        });

        if self.draft.is_some() {
            ui.add_space(6.0);
            self.editor(ui, notify);
        }

        ui.add_space(8.0);
        let mut edit: Option<Draft> = None;
        let mut remove: Option<(Mods, Key)> = None;
        egui::Grid::new("binds").striped(true).num_columns(3).spacing(Vec2::new(24.0, 6.0)).show(ui, |ui| {
            ui.label(RichText::new("Keys").strong());
            ui.label(RichText::new("Does").strong());
            ui.label("");
            ui.end_row();
            for b in &self.binds {
                ui.label(RichText::new(pretty_combo(b.mods, b.key)).monospace());
                let what = if b.description.is_empty() { b.cmd.describe() } else { b.description.clone() };
                match &b.submap {
                    Some(s) => ui.label(format!("{what}  (in mode \"{s}\")")),
                    None => ui.label(what),
                };
                ui.horizontal(|ui| {
                    if b.submap.is_none() {
                        if ui.small_button("Change").clicked() {
                            edit = Some(Draft::from_bind(b));
                        }
                        if ui.small_button("Remove").clicked() {
                            remove = Some((b.mods, b.key));
                        }
                    }
                });
                ui.end_row();
            }
        });
        if let Some(d) = edit {
            self.draft = Some(d);
        }
        if let Some(c) = remove {
            let mut f = BindsFile::load();
            f.remove(c);
            match self.save(&f) {
                Ok(()) => notify(format!("Removed {}", pretty_combo(c.0, c.1)), false),
                Err(e) => notify(format!("Couldn't save: {e}"), true),
            }
        }
    }

    fn editor(&mut self, ui: &mut egui::Ui, notify: &mut dyn FnMut(String, bool)) {
        let binds = self.binds.clone();
        let Some(d) = self.draft.as_mut() else { return };
        let mut close = false;
        let mut save: Option<BindLine> = None;
        egui::Frame::group(ui.style()).inner_margin(12.0).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(if d.original.is_some() { "Change keybind" } else { "New keybind" }).strong().size(15.0));
            ui.horizontal(|ui| {
                ui.label("Keys:");
                ui.checkbox(&mut d.sup, "Super");
                ui.checkbox(&mut d.ctrl, "Ctrl");
                ui.checkbox(&mut d.alt, "Alt");
                ui.checkbox(&mut d.shift, "Shift");
                ui.label("+");
                let mut keys = key_choices();
                if !keys.contains(&d.key) {
                    keys.insert(0, d.key.clone()); // e.g. a mouse button
                }
                let label = Key::parse(&d.key).map(|k| k.label()).unwrap_or_else(|_| d.key.clone());
                egui::ComboBox::from_id_salt("key").selected_text(label).height(400.0).show_ui(ui, |ui| {
                    for k in keys {
                        let l = Key::parse(&k).map(|x| x.label()).unwrap_or_else(|_| k.clone());
                        ui.selectable_value(&mut d.key, k, l);
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.label("Does:");
                let current = d.action.map(|i| ACTIONS[i].label).unwrap_or("Custom (Hyprland dispatcher)");
                egui::ComboBox::from_id_salt("action").selected_text(current).width(300.0).height(400.0).show_ui(ui, |ui| {
                    for (i, a) in ACTIONS.iter().enumerate() {
                        if ui.selectable_value(&mut d.action, Some(i), a.label).clicked() {
                            d.arg = match a.arg {
                                ArgKind::Workspace => "1".into(),
                                ArgKind::Direction => "l".into(),
                                _ => String::new(),
                            };
                        }
                    }
                    ui.selectable_value(&mut d.action, None, "Custom (Hyprland dispatcher)");
                });
                match d.action.map(|i| ACTIONS[i].arg) {
                    Some(ArgKind::Workspace) => {
                        let mut n: u32 = d.arg.parse().unwrap_or(1);
                        ui.add(egui::DragValue::new(&mut n).range(1..=10).prefix("workspace "));
                        d.arg = n.to_string();
                    }
                    Some(ArgKind::Direction) => {
                        for (name, code) in DIRECTIONS {
                            ui.selectable_value(&mut d.arg, code.to_string(), *name);
                        }
                    }
                    Some(ArgKind::Text(hint)) => {
                        ui.add(egui::TextEdit::singleline(&mut d.arg).hint_text(hint).desired_width(260.0));
                    }
                    Some(ArgKind::Fixed(_)) => {}
                    None => {
                        ui.add(egui::TextEdit::singleline(&mut d.custom_disp).hint_text("dispatcher, e.g. resizeactive").desired_width(180.0));
                        ui.add(egui::TextEdit::singleline(&mut d.custom_arg).hint_text("arguments, e.g. 20 0").desired_width(140.0));
                    }
                }
            });
            // What these keys do today (other than the bind being changed).
            let combo = Key::parse(&d.key).ok().map(|k| (d.mods(), k));
            if let Some(c) = combo {
                if Some(c) != d.original {
                    if let Some(other) = binds.iter().find(|b| b.submap.is_none() && (b.mods, b.key) == c) {
                        ui.label(RichText::new(format!("{} currently does \"{}\"; saving replaces it.", pretty_combo(c.0, c.1), other.cmd.describe())).color(Color32::from_rgb(240, 190, 60)));
                    }
                }
                if c.0 == Mods::SUPER && matches!(c.1, Key::Vk(0x4C)) {
                    ui.label(RichText::new("Windows always keeps Super + L for locking the screen.").weak());
                }
            }
            if let Some(e) = &d.error {
                ui.label(RichText::new(e).color(Color32::from_rgb(240, 110, 110)));
            }
            ui.horizontal(|ui| {
                if ui.button(RichText::new("Save").strong()).clicked() {
                    match d.line() {
                        Ok(l) => save = Some(l),
                        Err(e) => d.error = Some(e),
                    }
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if let Some(line) = save {
            let original = self.draft.as_ref().and_then(|d| d.original);
            let mut f = BindsFile::load();
            let label = pretty_combo(line.mods, line.key);
            f.set(original, line);
            match self.save(&f) {
                Ok(()) => {
                    notify(format!("Saved {label}"), false);
                    close = true;
                }
                Err(e) => notify(format!("Couldn't save: {e}"), true),
            }
        }
        if close {
            self.draft = None;
        }
    }
}
