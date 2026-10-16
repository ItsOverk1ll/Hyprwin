//! Hyprland's master layout.

use crate::WinId;
use crate::geom::Rect;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orientation {
    Left,
    Right,
    Top,
    Bottom,
    Center,
}

impl Orientation {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "left" => Self::Left,
            "right" => Self::Right,
            "top" => Self::Top,
            "bottom" => Self::Bottom,
            "center" => Self::Center,
            _ => return None,
        })
    }
    fn cycle(self, forward: bool) -> Self {
        const ALL: [Orientation; 5] = [Orientation::Left, Orientation::Top, Orientation::Right, Orientation::Bottom, Orientation::Center];
        let i = ALL.iter().position(|&o| o == self).unwrap_or(0) as i32;
        ALL[((i + if forward { 1 } else { -1 }).rem_euclid(5)) as usize]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NewStatus {
    Master,
    Slave,
    Inherit,
}

#[derive(Clone, Debug)]
pub struct MasterConfig {
    pub mfact: f64,
    pub new_status: NewStatus,
    pub new_on_top: bool,
    pub orientation: Orientation,
    pub slave_count_for_center_master: usize,
    pub allow_small_split: bool,
}

impl Default for MasterConfig {
    fn default() -> Self {
        Self {
            mfact: 0.55,
            new_status: NewStatus::Slave,
            new_on_top: false,
            orientation: Orientation::Left,
            slave_count_for_center_master: 2,
            allow_small_split: false,
        }
    }
}

#[derive(Clone, Debug)]
struct Item {
    win: WinId,
    master: bool,
    /// Relative size within its column/row (Hyprland `percSize`).
    weight: f64,
}

#[derive(Clone, Debug, Default)]
pub struct Master {
    items: Vec<Item>,
    /// Per-workspace overrides set by layoutmsg.
    mfact: Option<f64>,
    orientation: Option<Orientation>,
}

impl Master {
    pub fn to_json(&self) -> serde_json::Value {
        let items: Vec<serde_json::Value> = self.items.iter().map(|i| serde_json::json!({ "w": i.win, "master": i.master, "weight": i.weight })).collect();
        let orient = self.orientation.map(|o| format!("{o:?}").to_lowercase());
        serde_json::json!({ "items": items, "mfact": self.mfact, "orientation": orient })
    }

    pub fn from_json(v: &serde_json::Value, keep: &dyn Fn(WinId) -> bool) -> Master {
        let mut m = Master {
            items: Vec::new(),
            mfact: v["mfact"].as_f64(),
            orientation: v["orientation"].as_str().and_then(Orientation::parse),
        };
        for it in v["items"].as_array().into_iter().flatten() {
            let Some(w) = it["w"].as_u64() else { continue };
            if keep(w) && !m.contains(w) {
                m.items.push(Item { win: w, master: it["master"].as_bool().unwrap_or(false), weight: it["weight"].as_f64().unwrap_or(1.0) });
            }
        }
        if !m.items.is_empty() && !m.items.iter().any(|i| i.master) {
            m.items[0].master = true;
        }
        m
    }

    pub fn windows(&self) -> Vec<WinId> {
        self.items.iter().map(|i| i.win).collect()
    }
    pub fn contains(&self, w: WinId) -> bool {
        self.items.iter().any(|i| i.win == w)
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    fn idx(&self, w: WinId) -> Option<usize> {
        self.items.iter().position(|i| i.win == w)
    }

    pub fn insert(&mut self, w: WinId, focused: Option<WinId>, cfg: &MasterConfig) {
        if self.contains(w) {
            return;
        }
        let focused_is_master = focused.and_then(|f| self.idx(f)).map(|i| self.items[i].master).unwrap_or(false);
        let master = self.items.is_empty()
            || match cfg.new_status {
                NewStatus::Master => true,
                NewStatus::Slave => false,
                NewStatus::Inherit => focused_is_master,
            };
        if master {
            // A new master replaces the current single master unless several are allowed.
            if cfg.new_status == NewStatus::Master {
                for it in &mut self.items {
                    it.master = false;
                }
            }
            self.items.insert(0, Item { win: w, master: true, weight: 1.0 });
        } else if cfg.new_on_top {
            let first_slave = self.items.iter().position(|i| !i.master).unwrap_or(self.items.len());
            self.items.insert(first_slave, Item { win: w, master: false, weight: 1.0 });
        } else {
            self.items.push(Item { win: w, master: false, weight: 1.0 });
        }
    }

    pub fn remove(&mut self, w: WinId) -> bool {
        let Some(i) = self.idx(w) else { return false };
        let was_master = self.items.remove(i).master;
        if was_master && !self.items.iter().any(|i| i.master) {
            if let Some(first) = self.items.first_mut() {
                first.master = true;
            }
        }
        true
    }

    pub fn swap(&mut self, a: WinId, b: WinId) {
        let (Some(ia), Some(ib)) = (self.idx(a), self.idx(b)) else { return };
        let (wa, wb) = (self.items[ia].win, self.items[ib].win);
        self.items[ia].win = wb;
        self.items[ib].win = wa;
    }

    fn mfact(&self, cfg: &MasterConfig) -> f64 {
        self.mfact.unwrap_or(cfg.mfact).clamp(0.05, 0.95)
    }

    fn orientation(&self, cfg: &MasterConfig) -> Orientation {
        self.orientation.unwrap_or(cfg.orientation)
    }

    pub fn layout(&self, area: Rect, cfg: &MasterConfig) -> Vec<(WinId, Rect)> {
        let masters: Vec<&Item> = self.items.iter().filter(|i| i.master).collect();
        let slaves: Vec<&Item> = self.items.iter().filter(|i| !i.master).collect();
        let mut out = Vec::new();
        if slaves.is_empty() || masters.is_empty() {
            stack(&self.items.iter().collect::<Vec<_>>(), area, matches!(self.orientation(cfg), Orientation::Top | Orientation::Bottom), &mut out);
            return out;
        }
        let f = self.mfact(cfg);
        match self.orientation(cfg) {
            Orientation::Left | Orientation::Right => {
                let mw = (area.w as f64 * f).round() as i32;
                let left = self.orientation(cfg) == Orientation::Left;
                let (m, s) = if left {
                    (Rect::new(area.x, area.y, mw, area.h), Rect::new(area.x + mw, area.y, area.w - mw, area.h))
                } else {
                    (Rect::new(area.right() - mw, area.y, mw, area.h), Rect::new(area.x, area.y, area.w - mw, area.h))
                };
                stack(&masters, m, false, &mut out);
                stack(&slaves, s, false, &mut out);
            }
            Orientation::Top | Orientation::Bottom => {
                let mh = (area.h as f64 * f).round() as i32;
                let top = self.orientation(cfg) == Orientation::Top;
                let (m, s) = if top {
                    (Rect::new(area.x, area.y, area.w, mh), Rect::new(area.x, area.y + mh, area.w, area.h - mh))
                } else {
                    (Rect::new(area.x, area.bottom() - mh, area.w, mh), Rect::new(area.x, area.y, area.w, area.h - mh))
                };
                stack(&masters, m, true, &mut out);
                stack(&slaves, s, true, &mut out);
            }
            Orientation::Center => {
                if slaves.len() < cfg.slave_count_for_center_master {
                    // Falls back to left orientation, as Hyprland's center_master_fallback = left.
                    let mw = (area.w as f64 * f).round() as i32;
                    stack(&masters, Rect::new(area.x, area.y, mw, area.h), false, &mut out);
                    stack(&slaves, Rect::new(area.x + mw, area.y, area.w - mw, area.h), false, &mut out);
                } else {
                    let mw = (area.w as f64 * f).round() as i32;
                    let side = (area.w - mw) / 2;
                    let right: Vec<&Item> = slaves.iter().step_by(2).copied().collect();
                    let left: Vec<&Item> = slaves.iter().skip(1).step_by(2).copied().collect();
                    stack(&left, Rect::new(area.x, area.y, side, area.h), false, &mut out);
                    stack(&masters, Rect::new(area.x + side, area.y, mw, area.h), false, &mut out);
                    stack(&right, Rect::new(area.x + side + mw, area.y, area.w - side - mw, area.h), false, &mut out);
                }
            }
        }
        out
    }

    /// Returns false when the message is not a master layoutmsg.
    pub fn message(&mut self, msg: &str, focused: Option<WinId>, cfg: &MasterConfig) -> bool {
        let mut parts = msg.split_whitespace();
        let cmd = parts.next().unwrap_or("");
        let arg = parts.next();
        match cmd {
            "swapwithmaster" => {
                if let (Some(f), Some(m)) = (focused, self.items.iter().find(|i| i.master).map(|i| i.win)) {
                    if f == m {
                        if let Some(s) = self.items.iter().find(|i| !i.master).map(|i| i.win) {
                            self.swap(f, s);
                        }
                    } else {
                        self.swap(f, m);
                    }
                }
            }
            "addmaster" => {
                if let Some(i) = self.items.iter().position(|i| !i.master) {
                    if self.items.iter().filter(|i| !i.master).count() > 1 || cfg.allow_small_split {
                        self.items[i].master = true;
                    }
                }
            }
            "removemaster" => {
                if self.items.iter().filter(|i| i.master).count() > 1 {
                    if let Some(i) = self.items.iter().rposition(|i| i.master) {
                        self.items[i].master = false;
                    }
                }
            }
            "mfact" => {
                if let Some(a) = arg {
                    let exact = a == "exact";
                    let v = if exact { parts.next() } else { Some(a) };
                    if let Some(v) = v.and_then(|v| v.parse::<f64>().ok()) {
                        let cur = self.mfact(cfg);
                        self.mfact = Some(if exact { v } else { cur + v }.clamp(0.05, 0.95));
                    }
                }
            }
            "orientationleft" => self.orientation = Some(Orientation::Left),
            "orientationright" => self.orientation = Some(Orientation::Right),
            "orientationtop" => self.orientation = Some(Orientation::Top),
            "orientationbottom" => self.orientation = Some(Orientation::Bottom),
            "orientationcenter" => self.orientation = Some(Orientation::Center),
            "orientationnext" => self.orientation = Some(self.orientation(cfg).cycle(true)),
            "orientationprev" => self.orientation = Some(self.orientation(cfg).cycle(false)),
            "rollnext" if !self.items.is_empty() => {
                let w: Vec<WinId> = self.windows();
                let n = w.len();
                for (i, it) in self.items.iter_mut().enumerate() {
                    it.win = w[(i + 1) % n];
                }
            }
            "rollprev" if !self.items.is_empty() => {
                let w: Vec<WinId> = self.windows();
                let n = w.len();
                for (i, it) in self.items.iter_mut().enumerate() {
                    it.win = w[(i + n - 1) % n];
                }
            }
            _ => return false,
        }
        true
    }

    /// Next/previous window in master order (for `cyclenext`/`swapnext`).
    pub fn neighbour_in_order(&self, w: WinId, forward: bool) -> Option<WinId> {
        let i = self.idx(w)?;
        let n = self.items.len();
        Some(self.items[(i + if forward { 1 } else { n - 1 }) % n].win)
    }

    pub fn master_window(&self) -> Option<WinId> {
        self.items.iter().find(|i| i.master).map(|i| i.win)
    }

    /// Keyboard resize: horizontal deltas move the master edge (mfact), vertical
    /// deltas change the window's share of its column.
    pub fn resize(&mut self, w: WinId, dx: i32, dy: i32, area: Rect, cfg: &MasterConfig) {
        let Some(i) = self.idx(w) else { return };
        let horizontal_layout = matches!(self.orientation(cfg), Orientation::Left | Orientation::Right | Orientation::Center);
        let (along, across) = if horizontal_layout { (dx, dy) } else { (dy, dx) };
        if along != 0 {
            let extent = if horizontal_layout { area.w } else { area.h } as f64;
            let mut d = along as f64 / extent.max(1.0);
            let is_master = self.items[i].master;
            let flipped = matches!(self.orientation(cfg), Orientation::Right | Orientation::Bottom);
            if !is_master {
                d = -d;
            }
            if flipped {
                d = -d * if is_master { 1.0 } else { 1.0 };
            }
            let cur = self.mfact(cfg);
            self.mfact = Some((cur + d).clamp(0.05, 0.95));
        }
        if across != 0 {
            let peers = self.items.iter().filter(|x| x.master == self.items[i].master).count().max(1);
            let extent = if horizontal_layout { area.h } else { area.w } as f64;
            let d = across as f64 / (extent / peers as f64).max(1.0);
            self.items[i].weight = (self.items[i].weight + d).clamp(0.1, 10.0);
        }
    }
}

fn stack(items: &[&Item], area: Rect, horizontal: bool, out: &mut Vec<(WinId, Rect)>) {
    if items.is_empty() {
        return;
    }
    let total: f64 = items.iter().map(|i| i.weight).sum();
    let extent = if horizontal { area.w } else { area.h };
    let mut pos = 0;
    for (k, it) in items.iter().enumerate() {
        let size = if k + 1 == items.len() { extent - pos } else { (extent as f64 * it.weight / total).round() as i32 };
        let r = if horizontal { Rect::new(area.x + pos, area.y, size, area.h) } else { Rect::new(area.x, area.y + pos, area.w, size) };
        out.push((it.win, r));
        pos += size;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect::new(0, 0, 1000, 600);

    fn get(l: &[(WinId, Rect)], w: WinId) -> Rect {
        l.iter().find(|x| x.0 == w).unwrap().1
    }

    #[test]
    fn master_left_with_stack() {
        let c = MasterConfig::default();
        let mut m = Master::default();
        for w in 1..=3 {
            m.insert(w, None, &c);
        }
        let l = m.layout(AREA, &c);
        assert_eq!(get(&l, 1), Rect::new(0, 0, 550, 600));
        assert_eq!(get(&l, 2), Rect::new(550, 0, 450, 300));
        assert_eq!(get(&l, 3), Rect::new(550, 300, 450, 300));
    }

    #[test]
    fn single_window_fills() {
        let c = MasterConfig::default();
        let mut m = Master::default();
        m.insert(7, None, &c);
        assert_eq!(m.layout(AREA, &c), vec![(7, AREA)]);
    }

    #[test]
    fn new_status_master_takes_over() {
        let c = MasterConfig { new_status: NewStatus::Master, ..Default::default() };
        let mut m = Master::default();
        m.insert(1, None, &c);
        m.insert(2, None, &c);
        assert_eq!(m.master_window(), Some(2));
    }

    #[test]
    fn swapwithmaster_and_remove_master() {
        let c = MasterConfig::default();
        let mut m = Master::default();
        for w in 1..=3 {
            m.insert(w, None, &c);
        }
        assert!(m.message("swapwithmaster", Some(3), &c));
        assert_eq!(m.master_window(), Some(3));
        m.remove(3);
        assert!(m.master_window().is_some());
    }

    #[test]
    fn orientation_and_mfact_messages() {
        let c = MasterConfig::default();
        let mut m = Master::default();
        for w in 1..=2 {
            m.insert(w, None, &c);
        }
        m.message("orientationtop", None, &c);
        m.message("mfact exact 0.5", None, &c);
        let l = m.layout(AREA, &c);
        assert_eq!(get(&l, 1), Rect::new(0, 0, 1000, 300));
        assert_eq!(get(&l, 2), Rect::new(0, 300, 1000, 300));
    }

    #[test]
    fn center_master() {
        let c = MasterConfig { orientation: Orientation::Center, mfact: 0.5, ..Default::default() };
        let mut m = Master::default();
        for w in 1..=3 {
            m.insert(w, None, &c);
        }
        let l = m.layout(AREA, &c);
        assert_eq!(get(&l, 1), Rect::new(250, 0, 500, 600));
        assert_eq!(get(&l, 2).x, 750);
        assert_eq!(get(&l, 3).x, 0);
    }

    #[test]
    fn resize_moves_master_edge() {
        let c = MasterConfig::default();
        let mut m = Master::default();
        m.insert(1, None, &c);
        m.insert(2, None, &c);
        m.resize(1, 100, 0, AREA, &c);
        assert_eq!(get(&m.layout(AREA, &c), 1).w, 650);
        m.resize(2, 100, 0, AREA, &c); // growing the slave shrinks the master
        assert_eq!(get(&m.layout(AREA, &c), 1).w, 550);
    }
}
