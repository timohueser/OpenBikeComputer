//! The numbers of a fitted curve along its samples.

use super::network::hypot;
use crate::metres;

/// A spacing under LEAST_STEP_M counts as LEAST_STEP_M, so that OSM vertices almost on one point do
/// not blow up the differences.
pub(super) const LEAST_STEP_M: f64 = 0.5;

/// The weights at u of the first point, the first tangent, the last point and the last tangent of a
/// cubic Hermite curve.
pub(super) fn hermite(u: f64) -> [f64; 4] {
    let (u2, u3) = (u * u, u * u * u);
    [2.0 * u3 - 3.0 * u2 + 1.0, u3 - 2.0 * u2 + u, 3.0 * u2 - 2.0 * u3, u3 - u2]
}

/// The length of the curve `chord` metres long, with the unit tangents t0 and t1, by Simpson's rule.
pub(super) fn arc(chord: [f64; 2], t0: [f64; 2], t1: [f64; 2]) -> f64 {
    let c = hypot(chord);
    let speed = |u: f64| {
        let (d0, d1, d2) = (3.0 * u * u - 4.0 * u + 1.0, 6.0 * u - 6.0 * u * u, 3.0 * u * u - 2.0 * u);
        hypot([d1 * chord[0] + c * (d0 * t0[0] + d2 * t1[0]), d1 * chord[1] + c * (d0 * t0[1] + d2 * t1[1])])
    };
    (speed(0.0) + 4.0 * speed(0.25) + 2.0 * speed(0.5) + 4.0 * speed(0.75) + speed(1.0)) / 12.0
}

/// The f that minimises Σ wᵢ(fᵢ − tᵢ)² + Σ λ·d·D², for each t of `targets`: D is the third divided
/// difference at the samples s, in metres, and d its span. A banded Cholesky solve.
pub(super) fn penalised<const N: usize>(targets: [&[f64]; N], w: &[f64], lambda: &[f64], s: &[f64]) -> [Vec<f64>; N] {
    const M: usize = 3;
    let (n, width) = (w.len(), M + 1);
    let mut band = vec![0.0; n * width];
    for i in 0..n {
        band[i * width] = w[i];
    }
    let h = |i: usize| (s[i + 1] - s[i]).max(LEAST_STEP_M);
    let second = |i: usize| {
        let (h0, h1) = (h(i - 1), h(i));
        let (a0, a2) = (2.0 / ((h0 + h1) * h0), 2.0 / ((h0 + h1) * h1));
        [a0, -(a0 + a2), a2]
    };
    for i in 1..n.saturating_sub(2) {
        let ([a0, a1, a2], [b0, b1, b2]) = (second(i), second(i + 1));
        let d = (h(i - 1) + h(i) + h(i + 1)) / 3.0;
        let c = [-a0 / d, (b0 - a1) / d, (b1 - a2) / d, b2 / d];
        for p in 0..c.len() {
            for q in 0..=p {
                band[(i - 1 + p) * width + p - q] += lambda[i] * d * c[p] * c[q];
            }
        }
    }
    // L·Lᵀ in place: band[i·width + d] holds L[i][i − d].
    for i in 0..n {
        let from = i.saturating_sub(M);
        for j in from..=i {
            let mut sum = band[i * width + i - j];
            for k in from..j {
                sum -= band[i * width + i - k] * band[j * width + j - k];
            }
            band[i * width + i - j] = if j == i { sum.max(1e-300).sqrt() } else { sum / band[j * width] };
        }
    }
    targets.map(|t| {
        let mut f: Vec<f64> = (0..n).map(|i| w[i] * t[i]).collect();
        for i in 0..n {
            let mut sum = f[i];
            for k in i.saturating_sub(M)..i {
                sum -= band[i * width + i - k] * f[k];
            }
            f[i] = sum / band[i * width];
        }
        for i in (0..n).rev() {
            let mut sum = f[i];
            for k in i + 1..=(i + M).min(n - 1) {
                sum -= band[k * width + k - i] * f[k];
            }
            f[i] = sum / band[i * width];
        }
        f
    })
}

/// A mean of `v` over ±`half` metres of `s`, with weights that taper linearly: a triangle filter.
pub(super) fn smooth_along(v: &[f64], s: &[f64], half: f64) -> Vec<f64> {
    let (mut lo, mut hi) = (0, 0);
    (0..v.len())
        .map(|i| {
            while s[lo] < s[i] - half {
                lo += 1;
            }
            while hi + 1 < v.len() && s[hi + 1] <= s[i] + half {
                hi += 1;
            }
            let (mut sum, mut weight) = (0.0, 0.0);
            for j in lo..=hi {
                let share = 1.0 - (s[j] - s[i]).abs() / half;
                sum += share * v[j];
                weight += share;
            }
            if weight > 0.0 {
                sum / weight
            } else {
                v[i]
            }
        })
        .collect()
}

/// The derivative of `f` over `s` at each sample: of the parabola through it and its neighbours,
/// one-sided at the ends.
pub(super) fn bessel(f: &[f64], s: &[f64]) -> Vec<f64> {
    let n = f.len();
    let slope = |i: usize| (f[i + 1] - f[i]) / (s[i + 1] - s[i]).max(1e-9);
    (0..n)
        .map(|i| match i {
            _ if n < 2 => 0.0,
            0 => slope(0),
            _ if i == n - 1 => slope(n - 2),
            _ => {
                let (h0, h1) = ((s[i] - s[i - 1]).max(1e-9), (s[i + 1] - s[i]).max(1e-9));
                (h1 * slope(i - 1) + h0 * slope(i)) / (h0 + h1)
            }
        })
        .collect()
}

/// Metres along the samples.
pub(super) fn lengths(x: &[f64], y: &[f64]) -> Vec<f64> {
    let mut s = vec![0.0; x.len()];
    for i in 1..x.len() {
        let d = [x[i] - x[i - 1], y[i] - y[i - 1]];
        s[i] = s[i - 1] + hypot(d) * metres(y[i - 1] + d[1] / 2.0);
    }
    s
}

/// Unit tangents along the samples.
pub(super) fn tangents(x: &[f64], y: &[f64], s: &[f64]) -> Vec<[f64; 2]> {
    let (dx, dy) = (bessel(x, s), bessel(y, s));
    dx.iter()
        .zip(&dy)
        .map(|(&a, &b)| {
            let l = a.hypot(b);
            if l > 0.0 {
                [a / l, b / l]
            } else {
                [1.0, 0.0]
            }
        })
        .collect()
}
