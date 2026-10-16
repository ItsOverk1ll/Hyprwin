/// A CSS-style cubic bezier easing curve with fixed endpoints (0,0) and (1,1),
/// as declared by Hyprland's `bezier = NAME, x0, y0, x1, y1`.
///
/// `y` values may leave [0, 1] (overshoot curves); `x` values are clamped to
/// [0, 1] so the curve stays a function of time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bezier {
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
}

impl Bezier {
    pub fn new(x1: f64, y1: f64, x2: f64, y2: f64) -> Self {
        Self { x1: x1.clamp(0.0, 1.0), y1, x2: x2.clamp(0.0, 1.0), y2 }
    }

    pub const LINEAR: Bezier = Bezier { x1: 0.0, y1: 0.0, x2: 1.0, y2: 1.0 };

    /// Hyprland's built-in `default` curve.
    pub const HYPR_DEFAULT: Bezier = Bezier { x1: 0.0, y1: 0.75, x2: 0.15, y2: 1.0 };

    fn coord(t: f64, p1: f64, p2: f64) -> f64 {
        // B(t) = 3(1-t)^2 t p1 + 3(1-t) t^2 p2 + t^3
        let u = 1.0 - t;
        3.0 * u * u * t * p1 + 3.0 * u * t * t * p2 + t * t * t
    }

    fn coord_deriv(t: f64, p1: f64, p2: f64) -> f64 {
        let u = 1.0 - t;
        3.0 * u * u * p1 + 6.0 * u * t * (p2 - p1) + 3.0 * t * t * (1.0 - p2)
    }

    /// Eased value for linear time `x` in [0, 1].
    pub fn ease(&self, x: f64) -> f64 {
        if x <= 0.0 {
            return 0.0;
        }
        if x >= 1.0 {
            return 1.0;
        }
        let t = self.solve_t(x);
        Self::coord(t, self.y1, self.y2)
    }

    /// Finds the curve parameter t whose x-coordinate equals `x`.
    /// Newton's method first, bisection as a guaranteed fallback.
    fn solve_t(&self, x: f64) -> f64 {
        let mut t = x;
        for _ in 0..8 {
            let err = Self::coord(t, self.x1, self.x2) - x;
            if err.abs() < 1e-7 {
                return t;
            }
            let d = Self::coord_deriv(t, self.x1, self.x2);
            if d.abs() < 1e-6 {
                break;
            }
            t -= err / d;
        }
        let (mut lo, mut hi) = (0.0, 1.0);
        t = x;
        for _ in 0..40 {
            let v = Self::coord(t, self.x1, self.x2);
            if (v - x).abs() < 1e-7 {
                break;
            }
            if v < x {
                lo = t;
            } else {
                hi = t;
            }
            t = (lo + hi) * 0.5;
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_are_fixed() {
        for b in [Bezier::LINEAR, Bezier::HYPR_DEFAULT, Bezier::new(0.05, 0.9, 0.1, 1.1)] {
            assert_eq!(b.ease(0.0), 0.0);
            assert_eq!(b.ease(1.0), 1.0);
        }
    }

    #[test]
    fn linear_is_identity() {
        for i in 0..=100 {
            let x = i as f64 / 100.0;
            assert!((Bezier::LINEAR.ease(x) - x).abs() < 1e-6);
        }
    }

    #[test]
    fn matches_css_ease_reference() {
        // CSS `ease` = cubic-bezier(0.25, 0.1, 0.25, 1.0); browser reference value at 0.5 ≈ 0.8024
        let ease = Bezier::new(0.25, 0.1, 0.25, 1.0);
        assert!((ease.ease(0.5) - 0.8024).abs() < 1e-3, "{}", ease.ease(0.5));
    }

    #[test]
    fn overshoot_curve_exceeds_one() {
        let overshoot = Bezier::new(0.05, 0.9, 0.1, 1.1);
        let max = (0..=1000).map(|i| overshoot.ease(i as f64 / 1000.0)).fold(f64::MIN, f64::max);
        assert!(max > 1.0);
    }

    #[test]
    fn hypr_default_is_monotonic() {
        let mut last = 0.0;
        for i in 0..=1000 {
            let v = Bezier::HYPR_DEFAULT.ease(i as f64 / 1000.0);
            assert!(v + 1e-9 >= last);
            last = v;
        }
    }
}
