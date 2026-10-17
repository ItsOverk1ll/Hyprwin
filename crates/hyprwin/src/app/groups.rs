//! App Groups: save the apps on a workspace and their arrangement, then
//! re-open them on demand (launching what isn't running) and arrange them
//! exactly as saved. Window *contents* are not saved, only apps and layout.
//!
//! (Hyprland's own "groups" are tabbed windows, a separate future feature.)

use std::time::{Duration, Instant};

use hyprwin_config::paths;
use hyprwin_layout::wm::NewWindow;
use hyprwin_layout::{Rect, WinId, WsId, WsSel, remap_layout_ids};
use serde_json::{Value, json};
use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};
use windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID;

use super::{App, hwnd};
use crate::{info, warn};

/// How long to wait for launched apps' windows before arranging what arrived.
const OPEN_TIMEOUT: Duration = Duration::from_secs(45);
/// Most apps one group may launch.
const MAX_GROUP_APPS: usize = 32;

#[derive(Clone, Debug)]
pub struct GroupApp {
    pub class: String,
    pub exe: String,
    pub aumid: Option<String>,
    pub title: String,
    pub floating: bool,
    pub float_rect: Option<Rect>,
}

#[derive(Debug)]
pub struct PendingGroup {
    pub name: String,
    pub ws: WsId,
    pub layout: Value,
    pub apps: Vec<GroupApp>,
    pub slots: Vec<Option<WinId>>,
    pub deadline: Instant,
}

/// The shell AppUserModelID of a window (how Store/UWP apps and many modern
/// apps such as Windows Terminal are launched), if it has one.
fn aumid(id: WinId) -> Option<String> {
    unsafe {
        let store: IPropertyStore = SHGetPropertyStoreForWindow(hwnd(id)).ok()?;
        let v = store.GetValue(&PKEY_AppUserModel_ID).ok()?;
        let s = PropVariantToStringAlloc(&v).ok()?;
        let out = s.to_string().ok().filter(|x| !x.is_empty());
        windows::Win32::System::Com::CoTaskMemFree(Some(s.0 as *const _));
        out
    }
}

fn group_path(name: &str) -> std::path::PathBuf {
    paths::groups_dir().join(format!("{}.json", paths::file_stem(name)))
}

pub fn list() -> Vec<(String, usize)> {
    let mut v: Vec<(String, usize)> = std::fs::read_dir(paths::groups_dir())
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
                .filter_map(|e| {
                    let g: Value = serde_json::from_str(&std::fs::read_to_string(e.path()).ok()?).ok()?;
                    Some((g["name"].as_str()?.to_string(), g["apps"].as_array().map(|a| a.len()).unwrap_or(0)))
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    v
}

pub fn delete(name: &str) -> Result<(), String> {
    std::fs::remove_file(group_path(name)).map_err(|e| format!("cannot delete group '{name}': {e}"))
}

fn launch(app: &GroupApp) {
    let r = if let Some(id) = &app.aumid {
        std::process::Command::new("explorer.exe").arg(format!("shell:AppsFolder\\{id}")).spawn().map(|_| ())
    } else if !app.exe.is_empty() {
        let exe = std::path::Path::new(&app.exe);
        let mut c = std::process::Command::new(exe);
        if let Some(dir) = exe.parent() {
            c.current_dir(dir);
        }
        c.spawn().map(|_| ())
    } else {
        Err(std::io::Error::other("no launch information saved"))
    };
    match r {
        Ok(()) => info!("app group: launched {} (\"{}\")", app.class, app.title),
        Err(e) => warn!("app group: cannot launch {} ({}): {e}", app.class, app.exe),
    }
}

impl App {
    /// Saves the apps on the focused workspace and their arrangement.
    pub fn save_group(&mut self, name: &str) -> Result<String, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("give the group a name".into());
        }
        let ws = self.wm.active_workspace();
        let tiled = self.wm.workspaces.get(&ws).map(|w| w.layout_windows()).unwrap_or_default();
        let floats: Vec<WinId> = self.wm.windows.values().filter(|w| w.ws == ws && w.floating && !w.minimized).map(|w| w.id).collect();
        let members: Vec<WinId> = tiled.iter().chain(floats.iter()).copied().filter(|id| !self.wm.windows[id].minimized).collect();
        if members.is_empty() {
            return Err("this workspace has no windows to save".into());
        }
        if members.len() > MAX_GROUP_APPS {
            return Err(format!("a group can hold at most {MAX_GROUP_APPS} apps; this workspace has {}", members.len()));
        }
        let mut apps = Vec::new();
        for &id in &members {
            let m = self.meta.get(&id).cloned().unwrap_or_default();
            let w = &self.wm.windows[&id];
            apps.push(json!({
                "class": m.class,
                "exe": m.exe,
                "aumid": aumid(id),
                "title": m.title,
                "floating": w.floating,
                "float_rect": [w.float_rect.x, w.float_rect.y, w.float_rect.w, w.float_rect.h],
            }));
        }
        // Only members may stay in the tree: a minimized window keeps its tile
        // there, and as a non-member it would be renumbered to slot 0 and
        // take the first app's place when the group is opened.
        let mut layout = self.wm.workspace_layout_json_keeping(ws, &|w| members.contains(&w));
        remap_layout_ids(&mut layout, &|w| members.iter().position(|&m| m == w).map(|i| i as u64));
        let doc = json!({ "version": 1, "name": name, "layout": layout, "apps": apps });
        std::fs::create_dir_all(paths::groups_dir()).map_err(|e| e.to_string())?;
        std::fs::write(group_path(name), serde_json::to_string_pretty(&doc).unwrap()).map_err(|e| e.to_string())?;
        info!("app group '{name}' saved with {} app(s)", members.len());
        Ok(format!("saved '{name}' ({} apps)", members.len()))
    }

    /// Opens a saved group on an empty workspace: reuses already-open windows
    /// of the same apps, launches the rest, and arranges them as saved.
    pub fn open_group(&mut self, name: &str) -> Result<String, String> {
        let text = std::fs::read_to_string(group_path(name)).map_err(|_| format!("no app group named '{name}'"))?;
        let doc: Value = serde_json::from_str(&text).map_err(|e| format!("group file is damaged: {e}"))?;
        let apps: Vec<GroupApp> = doc["apps"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|a| GroupApp {
                class: a["class"].as_str().unwrap_or("").to_string(),
                exe: a["exe"].as_str().unwrap_or("").to_string(),
                aumid: a["aumid"].as_str().map(str::to_string),
                title: a["title"].as_str().unwrap_or("").to_string(),
                floating: a["floating"].as_bool().unwrap_or(false),
                float_rect: a["float_rect"].as_array().filter(|r| r.len() == 4).map(|r| {
                    let n = |i: usize| r[i].as_i64().unwrap_or(0) as i32;
                    Rect::new(n(0), n(1), n(2), n(3))
                }),
            })
            .collect();
        if apps.is_empty() {
            return Err("that group has no apps".into());
        }
        // Group files can be shared or hand-edited: never launch a flood.
        if apps.len() > MAX_GROUP_APPS {
            return Err(format!("that group lists {} apps; at most {MAX_GROUP_APPS} can be opened at once", apps.len()));
        }
        // A fresh workspace on the focused monitor.
        let (ws, _) = self.wm.resolve(&WsSel::Empty).ok_or("no free workspace")?;
        let fx = self.wm.switch_workspace(&WsSel::Id(ws));
        self.apply(fx);
        let mut pg = PendingGroup {
            name: doc["name"].as_str().unwrap_or(name).to_string(),
            ws,
            layout: doc["layout"].clone(),
            slots: vec![None; apps.len()],
            apps,
            deadline: Instant::now() + OPEN_TIMEOUT,
        };
        // Reuse open windows of the same app (single-window apps like Discord
        // or Spotify would not open a second window anyway).
        let mut claimed: Vec<WinId> = Vec::new();
        let mut launched = 0;
        for i in 0..pg.apps.len() {
            let class = pg.apps[i].class.clone();
            let existing = self
                .wm
                .windows
                .values()
                .filter(|w| !w.minimized && !claimed.contains(&w.id))
                .map(|w| w.id)
                .find(|id| self.meta.get(id).is_some_and(|m| m.class == class));
            match existing {
                Some(id) => {
                    claimed.push(id);
                    pg.slots[i] = Some(id);
                    let fx = self.wm.move_to_workspace(Some(id), &WsSel::Id(ws), false);
                    self.apply(fx);
                }
                None => {
                    launch(&pg.apps[i]);
                    launched += 1;
                }
            }
        }
        let msg = format!("opening '{}': {} reused, {} launching", pg.name, pg.apps.len() - launched, launched);
        info!("app group {msg}");
        self.pending_groups.push(pg);
        self.check_groups();
        Ok(msg)
    }

    /// Called for every newly managed window: if a group is waiting for this
    /// app, the window joins the group's workspace. Returns the workspace.
    pub fn claim_for_group(&mut self, class: &str) -> Option<(usize, usize, WsId)> {
        for (gi, pg) in self.pending_groups.iter().enumerate() {
            if let Some(i) = pg.apps.iter().enumerate().position(|(i, a)| a.class == class && pg.slots[i].is_none()) {
                return Some((gi, i, pg.ws));
            }
        }
        None
    }

    pub fn fill_group_slot(&mut self, gi: usize, slot: usize, id: WinId) {
        if let Some(pg) = self.pending_groups.get_mut(gi) {
            pg.slots[slot] = Some(id);
        }
        self.check_groups();
    }

    /// Arranges every group that is complete or timed out.
    pub fn check_groups(&mut self) {
        let now = Instant::now();
        let ready: Vec<usize> = self
            .pending_groups
            .iter()
            .enumerate()
            .filter(|(_, pg)| pg.slots.iter().all(Option::is_some) || now >= pg.deadline)
            .map(|(i, _)| i)
            .rev()
            .collect();
        for i in ready {
            let pg = self.pending_groups.remove(i);
            let mut layout = pg.layout.clone();
            let slots = pg.slots.clone();
            remap_layout_ids(&mut layout, &|slot| slots.get(slot as usize).copied().flatten());
            for (i, app) in pg.apps.iter().enumerate() {
                let Some(id) = pg.slots[i] else { continue };
                if self.wm.windows.get(&id).is_some_and(|w| w.floating != app.floating) {
                    let fx = self.wm.set_floating(Some(id), Some(app.floating));
                    self.apply(fx);
                }
                if let (true, Some(r)) = (app.floating, app.float_rect) {
                    if let Some(w) = self.wm.windows.get_mut(&id) {
                        w.float_rect = r;
                    }
                }
            }
            let fx = self.wm.set_workspace_layout(pg.ws, &layout);
            self.apply(fx);
            let missing = pg.slots.iter().filter(|s| s.is_none()).count();
            if missing > 0 {
                warn!("app group '{}': {missing} app(s) did not open a window in time", pg.name);
            } else {
                info!("app group '{}' arranged", pg.name);
            }
        }
        if !self.pending_groups.is_empty() {
            unsafe {
                windows::Win32::UI::WindowsAndMessaging::SetTimer(Some(self.main), super::TIMER_GROUPS, 1000, None);
            }
        }
    }

    /// Adjusts a new window's placement if a pending group claims it.
    pub fn group_placement(&mut self, class: &str, verdict_is_normal: bool, nw: &mut NewWindow) -> Option<(usize, usize)> {
        if !verdict_is_normal {
            return None; // splash screens and dialogs never fill a group slot
        }
        let (gi, slot, ws) = self.claim_for_group(class)?;
        nw.workspace = Some(WsSel::Id(ws));
        nw.floating = self.pending_groups[gi].apps[slot].floating;
        Some((gi, slot))
    }
}
