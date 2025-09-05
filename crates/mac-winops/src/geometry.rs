#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CGPoint {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CGSize {
    pub width: f64,
    pub height: f64,
}

pub fn approx_eq_eps(a: f64, b: f64, eps: f64) -> bool {
    (a - b).abs() <= eps
}

pub fn rect_eq(p1: CGPoint, s1: CGSize, p2: CGPoint, s2: CGSize) -> bool {
    approx_eq_eps(p1.x, p2.x, 1.0)
        && approx_eq_eps(p1.y, p2.y, 1.0)
        && approx_eq_eps(s1.width, s2.width, 1.0)
        && approx_eq_eps(s1.height, s2.height, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approx_eq_eps_basic() {
        assert!(approx_eq_eps(1.0, 1.0, 0.0));
        assert!(approx_eq_eps(1.0, 1.000_5, 0.001));
        assert!(!approx_eq_eps(1.0, 1.01, 0.001));
    }
}
