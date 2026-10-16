//! Hyprland's dwindle layout: a binary space partition tree per workspace.

use crate::geom::{Dir, Rect, contains_point};
use crate::WinId;

#[derive(Clone, Debug)]
pub struct DwindleConfig {
    /// 0 = follow cursor, 1 = new window left/top, 2 = new window right/bottom.
    pub force_split: u8,
    pub preserve_split: bool,
    pub split_width_multiplier: f64,
    /// 1.0 = 50/50. Range 0.1–1.9 (fraction of the first child = ratio / 2).
    pub default_split_ratio: f64,
    pub use_active_for_splits: bool,
    pub permanent_direction_override: bool,
    pub special_scale_factor: f64,
}

impl Default for DwindleConfig {
    fn default() -> Self {
        Self {
            force_split: 0,
            preserve_split: false,
            split_width_multiplier: 1.0,
            default_split_ratio: 1.0,
            use_active_for_splits: true,
            permanent_direction_override: false,
            special_scale_factor: 1.0,
        }
    }
}

type NodeId = usize;

#[derive(Clone, Debug)]
enum Kind {
    Leaf(WinId),
    Split { children: [NodeId; 2], split_top: bool, ratio: f64, locked: bool },
}

#[derive(Clone, Debug)]
struct Node {
    parent: Option<NodeId>,
    kind: Kind,
}

#[derive(Clone, Debug, Default)]
pub struct Dwindle {
    nodes: Vec<Option<Node>>,
    free: Vec<NodeId>,
    root: Option<NodeId>,
    preselect: Option<Dir>,
}

impl Dwindle {
    fn alloc(&mut self, n: Node) -> NodeId {
        if let Some(i) = self.free.pop() {
            self.nodes[i] = Some(n);
            i
        } else {
            self.nodes.push(Some(n));
            self.nodes.len() - 1
        }
    }
    fn release(&mut self, i: NodeId) {
        self.nodes[i] = None;
        self.free.push(i);
    }
    fn node(&self, i: NodeId) -> &Node {
        self.nodes[i].as_ref().expect("live node")
    }
    fn node_mut(&mut self, i: NodeId) -> &mut Node {
        self.nodes[i].as_mut().expect("live node")
    }

    fn leaf_of(&self, w: WinId) -> Option<NodeId> {
        self.nodes.iter().position(|n| matches!(n, Some(Node { kind: Kind::Leaf(x), .. }) if *x == w))
    }

    pub fn contains(&self, w: WinId) -> bool {
        self.leaf_of(w).is_some()
    }

    pub fn is_empty(&self) -> bool {
        self.root.is_none()
    }

    /// Windows in tree order (left/top first).
    pub fn windows(&self) -> Vec<WinId> {
        let mut out = Vec::new();
        if let Some(r) = self.root {
            self.collect(r, &mut out);
        }
        out
    }

    fn collect(&self, n: NodeId, out: &mut Vec<WinId>) {
        match &self.node(n).kind {
            Kind::Leaf(w) => out.push(*w),
            Kind::Split { children, .. } => {
                self.collect(children[0], out);
                self.collect(children[1], out);
            }
        }
    }

    /// One-time override of the next split (`layoutmsg preselect <dir>`).
    pub fn preselect(&mut self, d: Option<Dir>) {
        self.preselect = d;
    }

    /// Inserts `w`, splitting `target` (or the last window if `target` is not
    /// in the tree). `cursor` is used by `force_split = 0`.
    pub fn insert(&mut self, w: WinId, target: Option<WinId>, cursor: Option<(f64, f64)>, area: Rect, cfg: &DwindleConfig) {
        if self.contains(w) {
            return;
        }
        let Some(root) = self.root else {
            self.root = Some(self.alloc(Node { parent: None, kind: Kind::Leaf(w) }));
            return;
        };
        let target_leaf = target
            .and_then(|t| self.leaf_of(t))
            .or_else(|| self.windows().last().and_then(|&t| self.leaf_of(t)))
            .unwrap_or(root);
        let boxes = self.node_boxes(area, cfg);
        let tbox = boxes.get(&target_leaf).copied().unwrap_or(area);
        let preselect = self.preselect;
        if !cfg.permanent_direction_override {
            self.preselect = None;
        }
        let (split_top, new_first) = match preselect {
            Some(d) => (!d.is_horizontal(), matches!(d, Dir::Left | Dir::Up)),
            None => {
                let split_top = tbox.h as f64 * cfg.split_width_multiplier > tbox.w as f64;
                let new_first = match cfg.force_split {
                    1 => true,
                    2 => false,
                    _ => match cursor {
                        Some((x, y)) if contains_point(tbox, x, y) => {
                            let (cx, cy) = tbox.center();
                            if split_top { y < cy } else { x < cx }
                        }
                        _ => false,
                    },
                };
                (split_top, new_first)
            }
        };
        self.split_leaf(target_leaf, w, split_top, new_first, cfg.default_split_ratio, preselect.is_some());
    }

    fn split_leaf(&mut self, leaf: NodeId, w: WinId, split_top: bool, new_first: bool, ratio: f64, locked: bool) {
        let parent = self.node(leaf).parent;
        let new_leaf = self.alloc(Node { parent: None, kind: Kind::Leaf(w) });
        let children = if new_first { [new_leaf, leaf] } else { [leaf, new_leaf] };
        let split = self.alloc(Node { parent, kind: Kind::Split { children, split_top, ratio: ratio.clamp(0.1, 1.9), locked } });
        self.node_mut(leaf).parent = Some(split);
        self.node_mut(new_leaf).parent = Some(split);
        match parent {
            None => self.root = Some(split),
            Some(p) => {
                if let Kind::Split { children, .. } = &mut self.node_mut(p).kind {
                    for c in children.iter_mut() {
                        if *c == leaf {
                            *c = split;
                        }
                    }
                }
            }
        }
    }

    pub fn remove(&mut self, w: WinId) -> bool {
        let Some(leaf) = self.leaf_of(w) else { return false };
        let parent = self.node(leaf).parent;
        self.release(leaf);
        let Some(p) = parent else {
            self.root = None;
            return true;
        };
        let sibling = match &self.node(p).kind {
            Kind::Split { children, .. } => if children[0] == leaf { children[1] } else { children[0] },
            Kind::Leaf(_) => unreachable!(),
        };
        let grand = self.node(p).parent;
        self.release(p);
        self.node_mut(sibling).parent = grand;
        match grand {
            None => self.root = Some(sibling),
            Some(g) => {
                if let Kind::Split { children, .. } = &mut self.node_mut(g).kind {
                    for c in children.iter_mut() {
                        if *c == p {
                            *c = sibling;
                        }
                    }
                }
            }
        }
        true
    }

    fn node_boxes(&self, area: Rect, cfg: &DwindleConfig) -> std::collections::HashMap<NodeId, Rect> {
        let mut out = std::collections::HashMap::new();
        if let Some(r) = self.root {
            self.walk(r, area, cfg, &mut out);
        }
        out
    }

    fn effective_split_top(&self, split_top: bool, b: Rect, cfg: &DwindleConfig) -> bool {
        if cfg.preserve_split { split_top } else { b.h as f64 * cfg.split_width_multiplier > b.w as f64 }
    }

    fn walk(&self, n: NodeId, b: Rect, cfg: &DwindleConfig, out: &mut std::collections::HashMap<NodeId, Rect>) {
        out.insert(n, b);
        if let Kind::Split { children, split_top, ratio, locked } = &self.node(n).kind {
            let top = if *locked { *split_top } else { self.effective_split_top(*split_top, b, cfg) };
            let f = ratio / 2.0;
            let (a, c) = if top {
                let h1 = (b.h as f64 * f).round() as i32;
                (Rect::new(b.x, b.y, b.w, h1), Rect::new(b.x, b.y + h1, b.w, b.h - h1))
            } else {
                let w1 = (b.w as f64 * f).round() as i32;
                (Rect::new(b.x, b.y, w1, b.h), Rect::new(b.x + w1, b.y, b.w - w1, b.h))
            };
            self.walk(children[0], a, cfg, out);
            self.walk(children[1], c, cfg, out);
        }
    }

    /// Raw window boxes (no inner gaps) for `area`.
    pub fn layout(&self, area: Rect, cfg: &DwindleConfig) -> Vec<(WinId, Rect)> {
        let boxes = self.node_boxes(area, cfg);
        let mut out: Vec<(WinId, Rect)> = boxes
            .iter()
            .filter_map(|(&n, &r)| match self.node(n).kind {
                Kind::Leaf(w) => Some((w, r)),
                _ => None,
            })
            .collect();
        let order = self.windows();
        out.sort_by_key(|(w, _)| order.iter().position(|x| x == w));
        out
    }

    /// `layoutmsg togglesplit`. Only meaningful with `preserve_split`, as in Hyprland.
    pub fn toggle_split(&mut self, w: WinId, area: Rect, cfg: &DwindleConfig) {
        let Some(leaf) = self.leaf_of(w) else { return };
        let Some(p) = self.node(leaf).parent else { return };
        let b = self.node_boxes(area, cfg).get(&p).copied().unwrap_or(area);
        let current = match self.node(p).kind {
            Kind::Split { split_top, locked, .. } => if locked { split_top } else { self.effective_split_top(split_top, b, cfg) },
            _ => return,
        };
        // Hyprland only honours togglesplit with preserve_split; we lock the
        // toggled split instead, so it also works without preserve_split.
        if let Kind::Split { split_top, locked, .. } = &mut self.node_mut(p).kind {
            *split_top = !current;
            *locked = true;
        }
    }

    /// `layoutmsg swapsplit`: swaps the two halves of the window's parent.
    pub fn swap_split(&mut self, w: WinId) {
        let Some(leaf) = self.leaf_of(w) else { return };
        let Some(p) = self.node(leaf).parent else { return };
        if let Kind::Split { children, .. } = &mut self.node_mut(p).kind {
            children.swap(0, 1);
        }
    }

    /// `layoutmsg splitratio [+|-]v [exact]`.
    pub fn split_ratio(&mut self, w: WinId, value: f64, exact: bool) {
        let Some(leaf) = self.leaf_of(w) else { return };
        let Some(p) = self.node(leaf).parent else { return };
        if let Kind::Split { ratio, .. } = &mut self.node_mut(p).kind {
            *ratio = if exact { value } else { *ratio + value }.clamp(0.1, 1.9);
        }
    }

    pub fn swap(&mut self, a: WinId, b: WinId) {
        let (Some(la), Some(lb)) = (self.leaf_of(a), self.leaf_of(b)) else { return };
        self.node_mut(la).kind = Kind::Leaf(b);
        self.node_mut(lb).kind = Kind::Leaf(a);
    }

    /// `movewindow <dir>` onto neighbour `target`: `w` is re-inserted next to
    /// `target`, on the side it moved towards.
    pub fn move_next_to(&mut self, w: WinId, target: WinId, dir: Dir, cfg: &DwindleConfig) {
        if w == target || !self.contains(target) || !self.remove(w) {
            return;
        }
        let leaf = self.leaf_of(target).expect("checked");
        let new_first = matches!(dir, Dir::Left | Dir::Up);
        self.split_leaf(leaf, w, !dir.is_horizontal(), new_first, cfg.default_split_ratio, true);
    }

    /// `resizeactive dx dy`: grows the window by moving the nearest shared edge.
    pub fn resize(&mut self, w: WinId, dx: i32, dy: i32, area: Rect, cfg: &DwindleConfig) {
        let boxes = self.node_boxes(area, cfg);
        for (delta, horizontal) in [(dx, true), (dy, false)] {
            if delta == 0 {
                continue;
            }
            let Some(mut child) = self.leaf_of(w) else { return };
            while let Some(p) = self.node(child).parent {
                let pb = boxes.get(&p).copied().unwrap_or(area);
                let (is_first, top) = match &self.node(p).kind {
                    Kind::Split { children, split_top, locked, .. } => {
                        (children[0] == child, if *locked { *split_top } else { self.effective_split_top(*split_top, pb, cfg) })
                    }
                    _ => unreachable!(),
                };
                if top != horizontal {
                    let extent = if horizontal { pb.w } else { pb.h } as f64;
                    let d = 2.0 * delta as f64 / extent.max(1.0);
                    if let Kind::Split { ratio, .. } = &mut self.node_mut(p).kind {
                        *ratio = (*ratio + if is_first { d } else { -d }).clamp(0.1, 1.9);
                    }
                    break;
                }
                child = p;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect::new(0, 0, 1600, 900);

    fn cfg() -> DwindleConfig {
        DwindleConfig::default()
    }

    #[test]
    fn first_window_fills_area() {
        let mut d = Dwindle::default();
        d.insert(1, None, None, AREA, &cfg());
        assert_eq!(d.layout(AREA, &cfg()), vec![(1, AREA)]);
    }

    #[test]
    fn second_window_splits_side_by_side_on_wide_area() {
        let mut d = Dwindle::default();
        d.insert(1, None, None, AREA, &cfg());
        d.insert(2, Some(1), None, AREA, &cfg());
        let l = d.layout(AREA, &cfg());
        assert_eq!(l, vec![(1, Rect::new(0, 0, 800, 900)), (2, Rect::new(800, 0, 800, 900))]);
    }

    #[test]
    fn third_window_dwindles_top_bottom() {
        let mut d = Dwindle::default();
        for (w, t) in [(1, None), (2, Some(1)), (3, Some(2))] {
            d.insert(w, t, None, AREA, &cfg());
        }
        let l = d.layout(AREA, &cfg());
        assert_eq!(l[1], (2, Rect::new(800, 0, 800, 450)));
        assert_eq!(l[2], (3, Rect::new(800, 450, 800, 450)));
    }

    #[test]
    fn cursor_decides_side_when_force_split_0() {
        let mut d = Dwindle::default();
        d.insert(1, None, None, AREA, &cfg());
        d.insert(2, Some(1), Some((100.0, 400.0)), AREA, &cfg()); // cursor on left half
        assert_eq!(d.windows(), vec![2, 1]);
    }

    #[test]
    fn force_split_overrides_cursor() {
        let c = DwindleConfig { force_split: 2, ..cfg() };
        let mut d = Dwindle::default();
        d.insert(1, None, None, AREA, &c);
        d.insert(2, Some(1), Some((100.0, 400.0)), AREA, &c);
        assert_eq!(d.windows(), vec![1, 2]);
    }

    #[test]
    fn remove_promotes_sibling() {
        let mut d = Dwindle::default();
        for (w, t) in [(1, None), (2, Some(1)), (3, Some(2))] {
            d.insert(w, t, None, AREA, &cfg());
        }
        d.remove(2);
        assert_eq!(d.layout(AREA, &cfg()), vec![(1, Rect::new(0, 0, 800, 900)), (3, Rect::new(800, 0, 800, 900))]);
        d.remove(1);
        d.remove(3);
        assert!(d.is_empty());
    }

    #[test]
    fn preserve_split_and_toggle() {
        let c = DwindleConfig { preserve_split: true, ..cfg() };
        let mut d = Dwindle::default();
        d.insert(1, None, None, AREA, &c);
        d.insert(2, Some(1), None, AREA, &c);
        d.toggle_split(2, AREA, &c);
        assert_eq!(d.layout(AREA, &c)[0].1, Rect::new(0, 0, 1600, 450));
        // A narrow area would normally force top/bottom, but the split is preserved.
        d.toggle_split(2, AREA, &c);
        let narrow = Rect::new(0, 0, 400, 1000);
        assert_eq!(d.layout(narrow, &c)[0].1, Rect::new(0, 0, 200, 1000));
    }

    #[test]
    fn split_ratio_and_resize() {
        let mut d = Dwindle::default();
        d.insert(1, None, None, AREA, &cfg());
        d.insert(2, Some(1), None, AREA, &cfg());
        d.split_ratio(1, 0.5, false); // 1.5 → first child 75%
        assert_eq!(d.layout(AREA, &cfg())[0].1.w, 1200);
        d.split_ratio(1, 1.0, true);
        d.resize(2, 100, 0, AREA, &cfg()); // window 2 is second: its left edge moves left
        assert_eq!(d.layout(AREA, &cfg())[1].1, Rect::new(700, 0, 900, 900));
        d.split_ratio(1, 5.0, true);
        assert_eq!(d.layout(AREA, &cfg())[0].1.w, 1520); // clamped to 1.9
    }

    #[test]
    fn preselect_is_one_shot() {
        let mut d = Dwindle::default();
        d.insert(1, None, None, AREA, &cfg());
        d.preselect(Some(Dir::Up));
        d.insert(2, Some(1), None, AREA, &cfg());
        assert_eq!(d.layout(AREA, &cfg())[0], (2, Rect::new(0, 0, 1600, 450)));
        d.insert(3, Some(1), None, AREA, &cfg());
        // back to automatic: 1 is 1600x450 → side by side
        assert_eq!(d.layout(AREA, &cfg()).iter().find(|x| x.0 == 3).unwrap().1, Rect::new(800, 450, 800, 450));
    }

    #[test]
    fn move_next_to_jumps_over_neighbour() {
        let mut d = Dwindle::default();
        d.insert(1, None, None, AREA, &cfg());
        d.insert(2, Some(1), None, AREA, &cfg());
        d.move_next_to(2, 1, Dir::Left, &cfg());
        assert_eq!(d.windows(), vec![2, 1]);
        d.swap(1, 2);
        assert_eq!(d.windows(), vec![1, 2]);
    }

    #[test]
    fn many_inserts_and_removes_keep_tree_consistent() {
        let mut d = Dwindle::default();
        for w in 1..=50u64 {
            d.insert(w, Some(w.saturating_sub(1)), None, AREA, &cfg());
        }
        for w in (1..=50u64).step_by(2) {
            assert!(d.remove(w));
        }
        let l = d.layout(AREA, &cfg());
        assert_eq!(l.len(), 25);
        let total: i64 = l.iter().map(|(_, r)| r.w as i64 * r.h as i64).sum();
        assert_eq!(total, AREA.w as i64 * AREA.h as i64, "boxes must tile the area exactly");
    }
}
