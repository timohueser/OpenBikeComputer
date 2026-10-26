//! Plane geometry in world coordinates, and the profiles sampled along lines.

use rstar::primitives::{GeomWithData, Rectangle};
use rstar::{RTree, AABB};

use crate::Part;

/// A point as a map key: equal only where the coordinates are equal to the bit.
pub fn key([x, y]: [f64; 2]) -> [u64; 2] {
    [x.to_bits(), y.to_bits()]
}

/// The box (x0, y0, x1, y1) of the points; x0 > x1 of none.
pub fn bounds<'a>(points: impl IntoIterator<Item = &'a [f64; 2]>) -> [f64; 4] {
    points.into_iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |b, &[x, y]| {
        [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)]
    })
}

/// Boxes (x0, y0, x1, y1) to find the ones that meet a box, edges included. A box of no points meets
/// nothing.
pub struct Boxes(RTree<GeomWithData<Rectangle<[f64; 2]>, usize>>);

impl Boxes {
    pub fn new(boxes: &[[f64; 4]]) -> Self {
        let items = boxes
            .iter()
            .enumerate()
            .filter(|(_, b)| b[0] <= b[2] && b[1] <= b[3])
            .map(|(i, b)| GeomWithData::new(Rectangle::from_corners([b[0], b[1]], [b[2], b[3]]), i));
        Self(RTree::bulk_load(items.collect()))
    }

    /// The numbers of the boxes that meet `b`, in order.
    pub fn overlapping(&self, b: [f64; 4]) -> Vec<usize> {
        let envelope = AABB::from_corners([b[0], b[1]], [b[2], b[3]]);
        let mut out: Vec<usize> = self.0.locate_in_envelope_intersecting(&envelope).map(|item| item.data).collect();
        out.sort_unstable();
        out
    }
}

/// Whether a point lies inside the rings of at least 3 points, even-odd.
pub fn contains(rings: &[&[[f64; 2]]], [x, y]: [f64; 2]) -> bool {
    let mut inside = false;
    for ring in rings.iter().filter(|r| r.len() >= 3) {
        for i in 0..ring.len() {
            let ([ax, ay], [bx, by]) = (ring[i], ring[(i + 1) % ring.len()]);
            if (ay > y) != (by > y) && x < ax + (y - ay) * (bx - ax) / (by - ay) {
                inside = !inside;
            }
        }
    }
    inside
}

/// The area of the rings of the parts, in squared world units: the outer rings less the holes.
pub fn area(parts: &[Part]) -> f64 {
    let ring = |r: &[[f64; 2]]| -> f64 {
        let n = r.len();
        (0..n).map(|i| r[i][0] * r[(i + 1) % n][1] - r[(i + 1) % n][0] * r[i][1]).sum::<f64>().abs() / 2.0
    };
    parts.iter().map(|p| if p.outer { ring(&p.points) } else { -ring(&p.points) }).sum()
}

/// The value at x of the line through the points (xp, fp), xp non-decreasing; the end values past
/// the ends, and the last value where x repeats.
pub fn interp(x: f64, xp: &[f64], fp: &[f64]) -> f64 {
    let n = xp.len();
    if x.is_nan() {
        return x;
    }
    if x < xp[0] {
        return fp[0];
    }
    if x >= xp[n - 1] {
        return fp[n - 1];
    }
    // xp[j] <= x < xp[j + 1].
    let j = xp.partition_point(|&v| v <= x) - 1;
    fp[j] + (fp[j + 1] - fp[j]) * (x - xp[j]) / (xp[j + 1] - xp[j])
}

/// start, start + step, … below stop.
pub fn arange(start: f64, stop: f64, step: f64) -> Vec<f64> {
    let len = ((stop - start) / step).ceil();
    if len.is_nan() || len <= 0.0 {
        return Vec::new();
    }
    (0..len as usize).map(|i| start + i as f64 * step).collect()
}

/// The q-th percentile of the values, linear between them; None of none.
pub fn percentile(values: &mut [f64], q: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let at = q / 100.0 * (values.len() - 1) as f64;
    let (lo, hi) = (at.floor() as usize, at.ceil() as usize);
    Some(values[lo] + (values[hi] - values[lo]) * (at - lo as f64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_profile_reads_between_its_points_and_holds_its_ends_past_them() {
        let (xp, fp) = ([0.0, 10.0, 10.0, 20.0], [0.0, 5.0, 7.0, 9.0]);
        let at = |x| interp(x, &xp, &fp);
        assert_eq!((at(-1.0), at(5.0), at(10.0), at(15.0), at(25.0)), (0.0, 2.5, 7.0, 8.0, 9.0));
        assert_eq!(interp(0.0, &[0.0, 0.0, 10.0], &[1.0, 2.0, 3.0]), 2.0);
        assert_eq!(arange(1.0, 2.0, 0.5), [1.0, 1.5]);
        assert_eq!(percentile(&mut [4.0, 1.0, 3.0, 2.0], 50.0), Some(2.5));
        assert_eq!(percentile(&mut [], 50.0), None);
    }
}
