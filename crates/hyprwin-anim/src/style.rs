use crate::lerp;

/// Integer rectangle in physical pixels (left, top, width, height).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }
    pub fn right(&self) -> i32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }
    pub fn center(&self) -> (f64, f64) {
        (self.x as f64 + self.w as f64 / 2.0, self.y as f64 + self.h as f64 / 2.0)
    }
    pub fn lerp(a: Rect, b: Rect, t: f64) -> Rect {
        Rect {
            x: lerp(a.x as f64, b.x as f64, t).round() as i32,
            y: lerp(a.y as f64, b.y as f64, t).round() as i32,
            w: lerp(a.w as f64, b.w as f64, t).round().max(1.0) as i32,
            h: lerp(a.h as f64, b.h as f64, t).round().max(1.0) as i32,
        }
    }
    /// Scales around the centre by `s`.
    pub fn scaled(&self, s: f64) -> Rect {
        let (cx, cy) = self.center();
        let w = (self.w as f64 * s).max(1.0);
        let h = (self.h as f64 * s).max(1.0);
        Rect::new((cx - w / 2.0).round() as i32, (cy - h / 2.0).round() as i32, w.round() as i32, h.round() as i32)
    }
    pub fn offset(&self, dx: i32, dy: i32) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.w, self.h)
    }
}

/// What to draw for one animated surface on one frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub rect: Rect,
    /// 0.0 – 1.0
    pub opacity: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// `animation = windows*, …, STYLE`
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WindowStyle {
    /// Slide in from the nearest monitor edge (or a forced direction).
    Slide(Option<Direction>),
    /// Grow from `start_percent` of the final size.
    Popin { start_percent: f64 },
    /// GNOME-style vertical squash/unsquash.
    Gnomed,
}

impl WindowStyle {
    /// Parses Hyprland style strings: `slide`, `slide left`, `popin`, `popin 80%`, `gnomed`.
    pub fn parse(s: &str) -> Option<WindowStyle> {
        let mut it = s.split_whitespace();
        match it.next()? {
            "slide" => Some(WindowStyle::Slide(it.next().and_then(parse_direction))),
            "popin" => {
                let pct = it.next().and_then(parse_percent).unwrap_or(80.0);
                Some(WindowStyle::Popin { start_percent: pct })
            }
            "gnomed" | "gnome" => Some(WindowStyle::Gnomed),
            _ => None,
        }
    }
}

/// `animation = workspaces*, …, STYLE`
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WorkspaceStyle {
    Slide,
    SlideVert,
    Fade,
    SlideFade { percent: f64 },
    SlideFadeVert { percent: f64 },
}

impl WorkspaceStyle {
    pub fn parse(s: &str) -> Option<WorkspaceStyle> {
        let mut it = s.split_whitespace();
        let name = it.next()?;
        let pct = it.next().and_then(parse_percent);
        match name {
            "slide" => Some(WorkspaceStyle::Slide),
            "slidevert" => Some(WorkspaceStyle::SlideVert),
            "fade" => Some(WorkspaceStyle::Fade),
            "slidefade" => Some(WorkspaceStyle::SlideFade { percent: pct.unwrap_or(20.0) }),
            "slidefadevert" => Some(WorkspaceStyle::SlideFadeVert { percent: pct.unwrap_or(20.0) }),
            _ => None,
        }
    }
}

fn parse_direction(s: &str) -> Option<Direction> {
    match s {
        "left" | "l" => Some(Direction::Left),
        "right" | "r" => Some(Direction::Right),
        "top" | "up" | "u" | "t" => Some(Direction::Up),
        "bottom" | "down" | "d" | "b" => Some(Direction::Down),
        _ => None,
    }
}

fn parse_percent(s: &str) -> Option<f64> {
    s.trim_end_matches('%').parse::<f64>().ok()
}

/// Frame for a window opening (`closing = false`) or closing (`closing = true`).
/// `eased` is the bezier output for linear progress (may overshoot).
pub fn window_frame(style: WindowStyle, target: Rect, monitor: Rect, eased: f64, closing: bool) -> Frame {
    // For closing we run the open animation backwards.
    let t = if closing { 1.0 - eased } else { eased };
    match style {
        WindowStyle::Popin { start_percent } => {
            let s = lerp(start_percent / 100.0, 1.0, t);
            Frame { rect: target.scaled(s), opacity: t.clamp(0.0, 1.0) }
        }
        WindowStyle::Gnomed => {
            let (_, cy) = target.center();
            let h = (target.h as f64 * t).max(1.0);
            let rect = Rect::new(target.x, (cy - h / 2.0).round() as i32, target.w, h.round() as i32);
            Frame { rect, opacity: t.clamp(0.0, 1.0) }
        }
        WindowStyle::Slide(dir) => {
            let dir = dir.unwrap_or_else(|| nearest_edge(target, monitor));
            let start = match dir {
                Direction::Left => target.offset(monitor.x - target.right(), 0),
                Direction::Right => target.offset(monitor.right() - target.x, 0),
                Direction::Up => target.offset(0, monitor.y - target.bottom()),
                Direction::Down => target.offset(0, monitor.bottom() - target.y),
            };
            Frame { rect: Rect::lerp(start, target, t), opacity: 1.0 }
        }
    }
}

fn nearest_edge(r: Rect, m: Rect) -> Direction {
    let (cx, cy) = r.center();
    let dl = cx - m.x as f64;
    let dr = m.right() as f64 - cx;
    let du = cy - m.y as f64;
    let dd = m.bottom() as f64 - cy;
    let min = dl.min(dr).min(du).min(dd);
    if min == dl {
        Direction::Left
    } else if min == dr {
        Direction::Right
    } else if min == du {
        Direction::Up
    } else {
        Direction::Down
    }
}

/// Frame for a window belonging to a workspace that is switching in or out.
/// `forward` = switching to a higher-numbered workspace (content moves left/up).
pub fn workspace_frame(style: WorkspaceStyle, rest: Rect, monitor: Rect, eased: f64, incoming: bool, forward: bool) -> Frame {
    let sign = if forward { 1.0 } else { -1.0 };
    // Offset as a fraction of the monitor size: incoming goes 1→0, outgoing 0→-1.
    let frac = if incoming { sign * (1.0 - eased) } else { -sign * eased };
    let fade_opacity = if incoming { eased } else { 1.0 - eased };
    match style {
        WorkspaceStyle::Slide => Frame { rect: rest.offset((frac * monitor.w as f64).round() as i32, 0), opacity: 1.0 },
        WorkspaceStyle::SlideVert => Frame { rect: rest.offset(0, (frac * monitor.h as f64).round() as i32), opacity: 1.0 },
        WorkspaceStyle::Fade => Frame { rect: rest, opacity: fade_opacity.clamp(0.0, 1.0) },
        WorkspaceStyle::SlideFade { percent } => Frame {
            rect: rest.offset((frac * monitor.w as f64 * percent / 100.0).round() as i32, 0),
            opacity: fade_opacity.clamp(0.0, 1.0),
        },
        WorkspaceStyle::SlideFadeVert { percent } => Frame {
            rect: rest.offset(0, (frac * monitor.h as f64 * percent / 100.0).round() as i32),
            opacity: fade_opacity.clamp(0.0, 1.0),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MON: Rect = Rect::new(0, 0, 3840, 2160);
    const WIN: Rect = Rect::new(100, 100, 800, 600);

    #[test]
    fn parses_hyprland_styles() {
        assert_eq!(WindowStyle::parse("popin 80%"), Some(WindowStyle::Popin { start_percent: 80.0 }));
        assert_eq!(WindowStyle::parse("slide left"), Some(WindowStyle::Slide(Some(Direction::Left))));
        assert_eq!(WindowStyle::parse("gnomed"), Some(WindowStyle::Gnomed));
        assert_eq!(WorkspaceStyle::parse("slidefade 30%"), Some(WorkspaceStyle::SlideFade { percent: 30.0 }));
        assert_eq!(WorkspaceStyle::parse("slidevert"), Some(WorkspaceStyle::SlideVert));
        assert_eq!(WorkspaceStyle::parse("bogus"), None);
    }

    #[test]
    fn open_animations_end_at_target() {
        for style in [WindowStyle::Popin { start_percent: 50.0 }, WindowStyle::Gnomed, WindowStyle::Slide(None)] {
            let f = window_frame(style, WIN, MON, 1.0, false);
            assert_eq!(f.rect, WIN, "{style:?}");
            assert_eq!(f.opacity, 1.0);
        }
    }

    #[test]
    fn slide_starts_offscreen_from_nearest_edge() {
        // WIN's centre (500, 400) is nearest the top edge.
        let f = window_frame(WindowStyle::Slide(None), WIN, MON, 0.0, false);
        assert!(f.rect.bottom() <= MON.y, "{:?}", f.rect);
        let left = Rect::new(10, 900, 400, 300);
        let f = window_frame(WindowStyle::Slide(None), left, MON, 0.0, false);
        assert!(f.rect.right() <= MON.x, "{:?}", f.rect);
    }

    #[test]
    fn workspace_slide_in_and_out() {
        let incoming0 = workspace_frame(WorkspaceStyle::Slide, WIN, MON, 0.0, true, true);
        assert_eq!(incoming0.rect.x, WIN.x + MON.w);
        let incoming1 = workspace_frame(WorkspaceStyle::Slide, WIN, MON, 1.0, true, true);
        assert_eq!(incoming1.rect, WIN);
        let outgoing1 = workspace_frame(WorkspaceStyle::Slide, WIN, MON, 1.0, false, true);
        assert_eq!(outgoing1.rect.x, WIN.x - MON.w);
        let back = workspace_frame(WorkspaceStyle::Slide, WIN, MON, 0.0, true, false);
        assert_eq!(back.rect.x, WIN.x - MON.w);
    }

    #[test]
    fn rect_lerp_never_collapses() {
        let r = Rect::lerp(Rect::new(0, 0, 0, 0), Rect::new(0, 0, 0, 0), 0.5);
        assert!(r.w >= 1 && r.h >= 1);
    }
}
