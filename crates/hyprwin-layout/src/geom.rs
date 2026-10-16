pub use hyprwin_anim::Rect;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

impl Dir {
    /// Hyprland direction tokens: l/r/u/d, left/right/up/down, t/b.
    pub fn parse(s: &str) -> Option<Dir> {
        match s.trim() {
            "l" | "left" => Some(Dir::Left),
            "r" | "right" => Some(Dir::Right),
            "u" | "up" | "t" | "top" => Some(Dir::Up),
            "d" | "down" | "b" | "bottom" => Some(Dir::Down),
            _ => None,
        }
    }
    pub fn is_horizontal(self) -> bool {
        matches!(self, Dir::Left | Dir::Right)
    }
}

/// CSS-style per-side gaps (Hyprland `gaps_out = 5,10,5,10`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Gaps {
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub left: i32,
}

impl Gaps {
    pub const fn all(v: i32) -> Self {
        Self { top: v, right: v, bottom: v, left: v }
    }
    /// Parses `5` / `5,10` / `5,10,15` / `5,10,15,20` (CSS order).
    pub fn parse(s: &str) -> Option<Gaps> {
        let v: Vec<i32> = s.split([',', ' ']).filter(|x| !x.is_empty()).map(|x| x.trim().parse().ok()).collect::<Option<_>>()?;
        Some(match v.as_slice() {
            [a] => Gaps::all(*a),
            [a, b] => Gaps { top: *a, right: *b, bottom: *a, left: *b },
            [a, b, c] => Gaps { top: *a, right: *b, bottom: *c, left: *b },
            [a, b, c, d] => Gaps { top: *a, right: *b, bottom: *c, left: *d },
            _ => return None,
        })
    }
    pub fn scaled(&self, s: f64) -> Gaps {
        let f = |v: i32| (v as f64 * s).round() as i32;
        Gaps { top: f(self.top), right: f(self.right), bottom: f(self.bottom), left: f(self.left) }
    }
}

pub fn shrink(r: Rect, g: Gaps) -> Rect {
    Rect::new(r.x + g.left, r.y + g.top, (r.w - g.left - g.right).max(1), (r.h - g.top - g.bottom).max(1))
}

pub fn contains_point(r: Rect, x: f64, y: f64) -> bool {
    x >= r.x as f64 && x < r.right() as f64 && y >= r.y as f64 && y < r.bottom() as f64
}

/// Applies Hyprland's inner gaps: every side that does not touch the outer
/// layout area is inset by `gaps_in` (so two neighbours are 2×gaps_in apart).
pub fn apply_inner_gaps(r: Rect, area: Rect, gaps_in: Gaps) -> Rect {
    let near = |a: i32, b: i32| (a - b).abs() <= 1;
    let l = if near(r.x, area.x) { 0 } else { gaps_in.left };
    let t = if near(r.y, area.y) { 0 } else { gaps_in.top };
    let rr = if near(r.right(), area.right()) { 0 } else { gaps_in.right };
    let b = if near(r.bottom(), area.bottom()) { 0 } else { gaps_in.bottom };
    shrink(r, Gaps { top: t, right: rr, bottom: b, left: l })
}

/// Finds the candidate rect that is the best directional neighbour of `from`,
/// like Hyprland's `movefocus`: candidates must lie in the direction, and the
/// closest one by edge distance wins, with perpendicular overlap preferred.
pub fn neighbor<T: Copy>(from: Rect, dir: Dir, candidates: &[(T, Rect)]) -> Option<T> {
    let (fcx, fcy) = from.center();
    let mut best: Option<(T, f64)> = None;
    for &(id, r) in candidates {
        if r == from {
            continue;
        }
        let (cx, cy) = r.center();
        // Must be on the correct side (centre beyond our centre and edge not fully behind).
        let ok = match dir {
            Dir::Left => cx < fcx && r.x < from.x,
            Dir::Right => cx > fcx && r.right() > from.right(),
            Dir::Up => cy < fcy && r.y < from.y,
            Dir::Down => cy > fcy && r.bottom() > from.bottom(),
        };
        if !ok {
            continue;
        }
        let (gap, overlap) = if dir.is_horizontal() {
            let gap = if dir == Dir::Left { from.x - r.right() } else { r.x - from.right() };
            let ov = (from.bottom().min(r.bottom()) - from.y.max(r.y)).max(0);
            (gap.max(0) as f64, ov as f64)
        } else {
            let gap = if dir == Dir::Up { from.y - r.bottom() } else { r.y - from.bottom() };
            let ov = (from.right().min(r.right()) - from.x.max(r.x)).max(0);
            (gap.max(0) as f64, ov as f64)
        };
        let perp = if dir.is_horizontal() { (cy - fcy).abs() } else { (cx - fcx).abs() };
        // Overlapping neighbours always beat non-overlapping ones.
        let score = gap + if overlap > 0.0 { 0.0 } else { 100_000.0 } + perp * 0.01;
        if best.as_ref().is_none_or(|b| score < b.1) {
            best = Some((id, score));
        }
    }
    best.map(|b| b.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gaps_parse_css() {
        assert_eq!(Gaps::parse("5"), Some(Gaps::all(5)));
        assert_eq!(Gaps::parse("5,10"), Some(Gaps { top: 5, right: 10, bottom: 5, left: 10 }));
        assert_eq!(Gaps::parse("1,2,3,4"), Some(Gaps { top: 1, right: 2, bottom: 3, left: 4 }));
        assert_eq!(Gaps::parse("x"), None);
    }

    #[test]
    fn neighbor_picks_closest_overlapping() {
        let me = Rect::new(500, 500, 100, 100);
        let c = [
            (1, Rect::new(300, 500, 100, 100)), // left, overlapping
            (2, Rect::new(380, 100, 100, 100)), // left-ish but above, closer x
            (3, Rect::new(700, 500, 100, 100)), // right
        ];
        assert_eq!(neighbor(me, Dir::Left, &c), Some(1));
        assert_eq!(neighbor(me, Dir::Right, &c), Some(3));
        assert_eq!(neighbor(me, Dir::Down, &c), None);
    }

    #[test]
    fn inner_gaps_only_between_windows() {
        let area = Rect::new(0, 0, 1000, 500);
        let left = apply_inner_gaps(Rect::new(0, 0, 500, 500), area, Gaps::all(5));
        let right = apply_inner_gaps(Rect::new(500, 0, 500, 500), area, Gaps::all(5));
        assert_eq!(left, Rect::new(0, 0, 495, 500));
        assert_eq!(right, Rect::new(505, 0, 495, 500));
        assert_eq!(right.x - left.right(), 10);
    }
}
