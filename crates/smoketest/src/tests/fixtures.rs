//! Shared smoketest helpers retained for active legacy tests.

use mac_winops::approx_eq_eps;

/// Approximate float equality within `eps` tolerance.
#[inline]
pub fn approx(a: f64, b: f64, eps: f64) -> bool {
    approx_eq_eps(a, b, eps)
}
