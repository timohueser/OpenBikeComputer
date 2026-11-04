//! Integer strokes and the booleans of the faces. A stroke is the union of a quad per segment, a pie
//! on the outer side of each inner vertex and a half disc at each round end. Every derived point is an
//! integer point plus a rounded offset, and arcs step at fixed global angles, so two cells that hold
//! the same points build the same rings.

use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay::{ContourDirection, IntOverlayOptions, Overlay};
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::core::solver::{Precision, Solver, Strategy};
use i_overlay::i_float::int::point::IntPoint;

use super::geom::{area2, round, P};

pub use i_overlay::core::overlay_rule::OverlayRule as Rule;

/// Rings with the NonZero rule: outer rings counter-clockwise, holes clockwise.
pub type Rings = Vec<Vec<P>>;

/// The arc step: 36 global directions.
pub const STEPS: i32 = 36;
/// An arc point closer than this to an arc's end (radians) is left out: no needle segment.
const END_EPS: f64 = 0.01;

/// One solver for every boolean: one path at every segment count, a snap radius of one unit, no
/// threads.
pub fn solver() -> Solver {
    Solver { strategy: Strategy::Frag, precision: Precision::ABSOLUTE, multithreading: None }
}

pub fn options() -> IntOverlayOptions<u64> {
    IntOverlayOptions {
        preserve_input_collinear: false,
        output_direction: ContourDirection::CounterClockwise,
        preserve_output_collinear: false,
        min_output_area: 0,
        ogc: false,
    }
}

/// `p + round(d · dir)`.
pub fn off(p: P, d: f64, dir: [f64; 2]) -> P {
    [p[0] + round(d * dir[0]) as i32, p[1] + round(d * dir[1]) as i32]
}

/// The unit vector of b − a, or None for a = b.
pub fn unit(a: P, b: P) -> Option<[f64; 2]> {
    let (x, y) = (f64::from(b[0] - a[0]), f64::from(b[1] - a[1]));
    let len = (x * x + y * y).sqrt();
    (len > 0.0).then(|| [x / len, y / len])
}

/// The left normal of a direction (x east, y south: a quarter turn from +x toward +y).
pub fn left(u: [f64; 2]) -> [f64; 2] {
    [-u[1], u[0]]
}

fn global_dir(k: i32) -> [f64; 2] {
    let a = f64::from(k.rem_euclid(STEPS)) * (2.0 * std::f64::consts::PI / f64::from(STEPS));
    [libm::cos(a), libm::sin(a)]
}

/// The pie about p of radius d from direction `from`, turning `sweep` radians (0 to 2π) toward +y:
/// p, the two ends at `from` and at the end direction, and the global arc directions between.
fn pie(p: P, d: f64, from: [f64; 2], to: [f64; 2], sweep: f64, out: &mut Vec<Vec<P>>) {
    if sweep <= 0.0 || d <= 0.0 {
        return;
    }
    let step = 2.0 * std::f64::consts::PI / f64::from(STEPS);
    let a0 = libm::atan2(from[1], from[0]);
    let mut ring = vec![p, off(p, d, from)];
    let mut k = libm::floor(a0 / step) as i32 + 1;
    loop {
        let a = f64::from(k) * step - a0;
        if a >= sweep - END_EPS {
            break;
        }
        if a > END_EPS {
            ring.push(off(p, d, global_dir(k)));
        }
        k += 1;
    }
    ring.push(off(p, d, to));
    push_positive(ring, out);
}

/// The fan about p from `from` at radius d0 turning `sweep` radians toward +y to radius d1: the
/// radius changes linearly with the turn, and the ends are the strokes' own corners.
pub fn pie_between(p: P, d0: f64, d1: f64, from: [f64; 2], to: [f64; 2], sweep: f64, out: &mut Vec<Vec<P>>) {
    if sweep <= 0.0 {
        return;
    }
    let step = 2.0 * std::f64::consts::PI / f64::from(STEPS);
    let a0 = libm::atan2(from[1], from[0]);
    let mut ring = vec![p, off(p, d0, from)];
    let mut k = libm::floor(a0 / step) as i32 + 1;
    loop {
        let a = f64::from(k) * step - a0;
        if a >= sweep - END_EPS {
            break;
        }
        if a > END_EPS {
            ring.push(off(p, d0 + (d1 - d0) * a / sweep, global_dir(k)));
        }
        k += 1;
    }
    ring.push(off(p, d1, to));
    push_positive(ring, out);
}

/// The disc about p of radius d: its points at the global directions.
pub fn disc(p: P, d: f64) -> Vec<P> {
    (0..STEPS).map(|k| off(p, d, global_dir(k))).collect()
}

/// Pushes a ring turned to a positive area; drops one with none.
pub fn push_positive(mut ring: Vec<P>, out: &mut Vec<Vec<P>>) {
    ring.dedup();
    if ring.len() > 1 && ring[0] == ring[ring.len() - 1] {
        ring.pop();
    }
    let a = area2(&ring);
    if a == 0 {
        return;
    }
    if a < 0 {
        ring.reverse();
    }
    out.push(ring);
}

/// The pieces of the stroke of a line with offset `d[i]` (units) at point i: a quad per segment, a
/// pie on the outer side of each inner vertex, and a half disc at each end where `round` says so.
/// `closed` strokes a ring: every vertex is inner.
pub fn pieces(p: &[P], d: &[f64], round_ends: [bool; 2], closed: bool, out: &mut Vec<Vec<P>>) {
    pieces_along(p, d, &[], round_ends, closed, out);
}

/// `pieces` where `dirs[i]`, if given, is the direction of segment i: a sample inserted on a segment
/// keeps the segment's own direction on both sides, so its sections meet the segment's.
pub fn pieces_along(
    p: &[P],
    d: &[f64],
    dirs: &[Option<[f64; 2]>],
    round_ends: [bool; 2],
    closed: bool,
    out: &mut Vec<Vec<P>>,
) {
    let n = p.len();
    if n == 0 {
        return;
    }
    let segs = if closed { n } else { n - 1 };
    // The direction of each segment; a zero segment takes none.
    let dirs: Vec<Option<[f64; 2]>> = (0..segs)
        .map(|i| {
            if p[i] == p[(i + 1) % n] {
                None
            } else {
                dirs.get(i).copied().flatten().or_else(|| unit(p[i], p[(i + 1) % n]))
            }
        })
        .collect();
    for i in 0..segs {
        let Some(u) = dirs[i] else { continue };
        let j = (i + 1) % n;
        let nl = left(u);
        let nr = [-nl[0], -nl[1]];
        push_positive(vec![off(p[i], d[i], nl), off(p[j], d[j], nl), off(p[j], d[j], nr), off(p[i], d[i], nr)], out);
    }
    // The joins: between each segment and the next one with a direction.
    let valid: Vec<usize> = (0..segs).filter(|&i| dirs[i].is_some()).collect();
    let pairs = valid.len().saturating_sub(usize::from(!closed));
    for k in 0..pairs {
        let (i, j) = (valid[k], valid[(k + 1) % valid.len()]);
        let (u1, u2) = (dirs[i].unwrap(), dirs[j].unwrap());
        let v = (i + 1) % n;
        let c = u1[0] * u2[1] - u1[1] * u2[0];
        let dot = u1[0] * u2[0] + u1[1] * u2[1];
        let turn = libm::atan2(c, dot);
        let (n1, n2) = (left(u1), left(u2));
        if turn > 0.0 || (turn == 0.0 && dot < 0.0) {
            // Turning toward the left normal: the outer side is the right one.
            pie(
                p[v],
                d[v],
                [-n1[0], -n1[1]],
                [-n2[0], -n2[1]],
                turn.abs().max(if dot < 0.0 && c == 0.0 { std::f64::consts::PI } else { 0.0 }),
                out,
            );
        } else if turn < 0.0 {
            pie(p[v], d[v], n2, n1, -turn, out);
        }
    }
    if !closed {
        if let (true, Some(&i)) = (round_ends[0], valid.first()) {
            let u = dirs[i].unwrap();
            let back = [-u[0], -u[1]];
            pie(p[0], d[0], left(back).map(|x| -x), left(back), std::f64::consts::PI, out);
        }
        if let (true, Some(&i)) = (round_ends[1], valid.last()) {
            let u = dirs[i].unwrap();
            pie(p[n - 1], d[n - 1], left(u).map(|x| -x), left(u), std::f64::consts::PI, out);
        }
    }
}

/// A boolean of two ring sets, NonZero, run in a frame about `anchor`, so that two cells that hold
/// the same rings about one global anchor get the same result.
pub fn boolean(subj: &[Vec<P>], clip: &[Vec<P>], rule: OverlayRule, anchor: P) -> Rings {
    shapes(subj, clip, rule, anchor).into_iter().flatten().collect()
}

/// A boolean whose result keeps its connected parts apart: each an outer ring and its holes.
pub fn shapes(subj: &[Vec<P>], clip: &[Vec<P>], rule: OverlayRule, anchor: P) -> Vec<Rings> {
    if subj.is_empty() && matches!(rule, OverlayRule::Subject | OverlayRule::Difference | OverlayRule::Intersect) {
        return Vec::new();
    }
    let shift = |rings: &[Vec<P>]| -> Vec<Vec<IntPoint>> {
        (rings.iter())
            .filter(|r| r.len() >= 3)
            .map(|r| r.iter().map(|p| IntPoint::new(p[0] - anchor[0], p[1] - anchor[1])).collect())
            .collect()
    };
    let (s, c) = (shift(subj), shift(clip));
    let out = Overlay::from_subj_and_clip_custom(&s, &c, options(), solver()).overlay(rule, FillRule::NonZero);
    out.into_iter()
        .map(|shape| {
            shape.into_iter().map(|c| c.into_iter().map(|q| [q.x + anchor[0], q.y + anchor[1]]).collect()).collect()
        })
        .collect()
}

pub fn union(rings: &[Vec<P>], anchor: P) -> Rings {
    boolean(rings, &[], OverlayRule::Subject, anchor)
}

/// The stroke of every ring's boundary at radius d.
fn boundary_pieces(region: &Rings, d: f64) -> Vec<Vec<P>> {
    let mut out = Vec::new();
    for ring in region {
        pieces(ring, &vec![d; ring.len()], [false, false], true, &mut out);
    }
    out
}

/// The region grown by d: the region and the stroke of its boundary.
pub fn dilate(region: &Rings, d: f64, anchor: P) -> Rings {
    let mut all = boundary_pieces(region, d);
    all.extend(region.iter().cloned());
    union(&all, anchor)
}

/// The region shrunk by d: the region less the stroke of its boundary.
pub fn erode(region: &Rings, d: f64, anchor: P) -> Rings {
    boolean(region, &boundary_pieces(region, d), OverlayRule::Difference, anchor)
}

/// The closing by radius r: what a disc of radius r cannot reach from outside. It holds the region.
pub fn closing(region: &Rings, r: f64, anchor: P) -> Rings {
    let grown = dilate(region, r, anchor);
    erode(&grown, r, anchor)
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_union_of_a_turn_fill_and_its_arm_pieces_completes() {
        let rings: Vec<Vec<[i32; 2]>> = vec![
            vec![
                [189391, 183582],
                [189355, 183601],
                [189505, 183496],
                [189561, 183440],
                [189967, 183440],
                [189752, 183459],
                [189541, 183516],
            ],
            vec![
                [189505, 181816],
                [189358, 181713],
                [189401, 181735],
                [189546, 181798],
                [189757, 181855],
                [189974, 181874],
                [191016, 181875],
                [189563, 181874],
            ],
            vec![[188736, 183751], [187643, 183687], [187771, 181497], [188864, 181561]],
            vec![
                [188800, 182656],
                [188864, 181561],
                [188990, 181576],
                [189175, 181626],
                [189348, 181706],
                [189505, 181816],
                [189640, 181951],
                [189750, 182108],
                [189830, 182281],
                [189880, 182466],
                [189896, 182656],
                [189880, 182846],
                [189830, 183031],
                [189750, 183204],
                [189640, 183361],
                [189505, 183496],
                [189348, 183606],
                [189175, 183686],
                [188990, 183736],
                [188800, 183752],
                [188736, 183751],
            ],
            vec![[188801, 181873], [190288, 181874], [190286, 183440], [188799, 183439]],
            vec![
                [188800, 182656],
                [188799, 183439],
                [188664, 183427],
                [188532, 183392],
                [188408, 183334],
                [188297, 183256],
                [188200, 183159],
                [188122, 183048],
                [188064, 182924],
                [188029, 182792],
                [188017, 182656],
                [188029, 182520],
                [188064, 182388],
                [188122, 182264],
                [188200, 182153],
                [188297, 182056],
                [188408, 181978],
                [188532, 181920],
                [188664, 181885],
                [188801, 181873],
            ],
        ];
        let u = super::union(&rings, [188800, 182656]);
        assert_eq!(u.len(), 1, "one outer ring and no hole: {u:?}");
    }
}
