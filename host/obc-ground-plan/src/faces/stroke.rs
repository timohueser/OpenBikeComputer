//! Integer strokes and the booleans of the faces. A stroke is the union of a quad per segment, a pie
//! on the outer side of each inner vertex and a half disc at each round end. Every derived point is an
//! integer point plus a rounded offset, and arcs step at fixed global angles, so two cells that hold
//! the same points build the same rings.

use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay::{ContourDirection, IntOverlayOptions, Overlay};
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::core::solver::{Precision, Solver, Strategy};
use i_overlay::i_float::int::point::IntPoint;
use i_overlay::string::overlay::StringOverlay;
use i_overlay::string::rule::StringRule;

use super::geom::{area2, round, P};

/// Rings with the NonZero rule: outer rings counter-clockwise, holes clockwise.
pub type Rings = Vec<Vec<P>>;

/// The arc step: 36 global directions.
pub const STEPS: i32 = 36;
/// A join that turns less than this (radians, one arc step) is mitred.
const MITRE: f64 = 2.0 * std::f64::consts::PI / STEPS as f64;

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

/// The corners of the narrower layers (offsets `cuts`, ascending) on the ray from p toward `dir`,
/// out to offset d: the vertices that a square end or a fan side of a wider layer shares with them.
fn ray(p: P, d: f64, dir: [f64; 2], cuts: &[f64]) -> impl DoubleEndedIterator<Item = P> + '_ {
    cuts.iter().filter(move |&&c| c > 0.0 && c < d).map(move |&c| off(p, c, dir))
}

/// The pie about p of radius d from direction `from`, turning `sweep` radians (0 to 2π) toward +y:
/// p, the two ends at `from` and at the end direction, and the global arc directions between. Its
/// two sides pass the corners of the narrower layers at offsets `cuts`.
fn pie(p: P, d: f64, from: [f64; 2], to: [f64; 2], sweep: f64, cuts: &[f64], out: &mut Vec<Vec<P>>) {
    if sweep <= 0.0 || d <= 0.0 {
        return;
    }
    let step = 2.0 * std::f64::consts::PI / f64::from(STEPS);
    let a0 = libm::atan2(from[1], from[0]);
    let mut ring = vec![p];
    ring.extend(ray(p, d, from, cuts));
    ring.push(off(p, d, from));
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
    ring.extend(ray(p, d, to, cuts).rev());
    push_positive(ring, out);
}

/// The fan about p from `from` at radius d0 turning `sweep` radians toward +y to radius d1: the
/// radius changes linearly with the turn, and the ends are the strokes' own corners. Its sides pass
/// the corners of the narrower layers at offsets `cuts` (at the start, at the end).
#[allow(clippy::too_many_arguments)]
pub fn pie_between(
    p: P,
    d0: f64,
    d1: f64,
    from: [f64; 2],
    to: [f64; 2],
    sweep: f64,
    cuts: [&[f64]; 2],
    out: &mut Vec<Vec<P>>,
) {
    if sweep <= 0.0 {
        return;
    }
    let step = 2.0 * std::f64::consts::PI / f64::from(STEPS);
    let a0 = libm::atan2(from[1], from[0]);
    let mut ring = vec![p];
    ring.extend(ray(p, d0, from, cuts[0]));
    ring.push(off(p, d0, from));
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
    ring.extend(ray(p, d1, to, cuts[1]).rev());
    push_positive(ring, out);
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
/// `closed` strokes a ring: every vertex is inner. `dirs[i]`, if given, is the direction of segment
/// i: a sample inserted on a segment keeps the segment's own direction on both sides, so its
/// sections meet the segment's. The end at the first and at the last point passes the corners at
/// offsets `cuts` (ascending) below d, so layers and ways that end on one line share its vertices.
///
/// An inner vertex of an open line whose inner mitre point would turn a segment's edge back (a
/// segment shorter than the offset times the turn) is left out: the edges about it then meet at one
/// mitre point. Else the two segments' own corners stand apart there, and where the offset changes
/// along them (a taper) the edge steps. The line moves by at most that segment's length times the
/// turn.
pub fn pieces_along(
    p: &[P],
    d: &[f64],
    dirs: &[Option<[f64; 2]>],
    cuts: [&[f64]; 2],
    round_ends: [bool; 2],
    closed: bool,
    out: &mut Vec<Vec<P>>,
) {
    let mut keep: Vec<usize> = (0..p.len()).collect();
    let mut k = 2;
    while !closed && k + 2 < keep.len() {
        let (a, v, b) = (keep[k - 1], keep[k], keep[k + 1]);
        if let (Some(u1), Some(u2)) = (unit(p[a], p[v]), unit(p[v], p[b])) {
            let c = u1[0] * u2[1] - u1[1] * u2[0];
            let dot = u1[0] * u2[0] + u1[1] * u2[1];
            let turn = libm::atan2(c, dot);
            if turn != 0.0 && turn.abs() < MITRE && dot > 0.0 {
                // Turning toward the left normal: the inner side is the left one.
                let s = if turn > 0.0 { 1.0 } else { -1.0 };
                let (n1, n2) = (left(u1).map(|x| s * x), left(u2).map(|x| s * x));
                let m = [(n1[0] + n2[0]) / (1.0 + dot), (n1[1] + n2[1]) / (1.0 + dot)];
                let q = off(p[v], d[v], m);
                let ahead =
                    |a: P, b: P, u: [f64; 2]| f64::from(b[0] - a[0]) * u[0] + f64::from(b[1] - a[1]) * u[1] > 0.0;
                if !(ahead(off(p[a], d[a], n1), q, u1) && ahead(q, off(p[b], d[b], n2), u2)) {
                    keep.remove(k);
                    k = (k - 1).max(2);
                    continue;
                }
            }
        }
        k += 1;
    }
    if keep.len() < p.len() {
        let p: Vec<P> = keep.iter().map(|&i| p[i]).collect();
        let d: Vec<f64> = keep.iter().map(|&i| d[i]).collect();
        let dirs: Vec<Option<[f64; 2]>> =
            keep.windows(2).map(|w| if w[1] == w[0] + 1 { dirs.get(w[0]).copied().flatten() } else { None }).collect();
        return stroke(&p, &d, &dirs, cuts, round_ends, closed, out);
    }
    stroke(p, d, dirs, cuts, round_ends, closed, out)
}

fn stroke(
    p: &[P],
    d: &[f64],
    dirs: &[Option<[f64; 2]>],
    cuts: [&[f64]; 2],
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
    // Per segment, its corners at start and end: left, right.
    let mut corners: Vec<[[P; 2]; 2]> = (0..segs)
        .map(|i| {
            let j = (i + 1) % n;
            let nl = dirs[i].map_or([0.0, 0.0], left);
            let nr = [-nl[0], -nl[1]];
            [[off(p[i], d[i], nl), off(p[i], d[i], nr)], [off(p[j], d[j], nl), off(p[j], d[j], nr)]]
        })
        .collect();
    // The joins: between each segment and the next one with a direction. A join that turns less than
    // one arc step takes one mitre point per side for both segments: no step between two corners. On
    // the inner side a mitre point that would turn either segment's edge back (a segment shorter than
    // the offset times the turn) is left out: the segments' own corners there overlap.
    let valid: Vec<usize> = (0..segs).filter(|&i| dirs[i].is_some()).collect();
    let pairs = valid.len().saturating_sub(usize::from(!closed));
    let mut pies = Vec::new();
    for k in 0..pairs {
        let (i, j) = (valid[k], valid[(k + 1) % valid.len()]);
        let (u1, u2) = (dirs[i].unwrap(), dirs[j].unwrap());
        let v = (i + 1) % n;
        let c = u1[0] * u2[1] - u1[1] * u2[0];
        let dot = u1[0] * u2[0] + u1[1] * u2[1];
        let turn = libm::atan2(c, dot);
        let (n1, n2) = (left(u1), left(u2));
        if turn.abs() < MITRE && dot > 0.0 {
            let m = [(n1[0] + n2[0]) / (1.0 + dot), (n1[1] + n2[1]) / (1.0 + dot)];
            let q = [off(p[v], d[v], m), off(p[v], d[v], [-m[0], -m[1]])];
            // Turning toward the left normal: the inner side is the left one.
            let inner = if turn > 0.0 { 0 } else { 1 };
            let ahead = |a: P, b: P, u: [f64; 2]| f64::from(b[0] - a[0]) * u[0] + f64::from(b[1] - a[1]) * u[1] > 0.0;
            for side in 0..2 {
                if side != inner || (ahead(corners[i][0][side], q[side], u1) && ahead(q[side], corners[j][1][side], u2))
                {
                    corners[i][1][side] = q[side];
                    corners[j][0][side] = q[side];
                }
            }
            continue;
        }
        pies.push((v, turn, dot, c, n1, n2));
    }
    let (first, last) = (valid.first().copied(), valid.last().copied());
    for i in 0..segs {
        let Some(u) = dirs[i] else { continue };
        let [[sl, sr], [el, er]] = corners[i];
        let nl = left(u);
        let nr = [-nl[0], -nl[1]];
        let j = (i + 1) % n;
        let mut ring = vec![el];
        if !closed && Some(i) == last {
            ring.extend(ray(p[j], d[j], nl, cuts[1]).rev());
            ring.extend(ray(p[j], d[j], nr, cuts[1]));
        }
        ring.extend([er, sr]);
        if !closed && Some(i) == first {
            ring.extend(ray(p[i], d[i], nr, cuts[0]).rev());
            ring.extend(ray(p[i], d[i], nl, cuts[0]));
        }
        ring.push(sl);
        push_positive(ring, out);
    }
    for (v, turn, dot, c, n1, n2) in pies {
        if turn > 0.0 || (turn == 0.0 && dot < 0.0) {
            // Turning toward the left normal: the outer side is the right one.
            pie(
                p[v],
                d[v],
                [-n1[0], -n1[1]],
                [-n2[0], -n2[1]],
                turn.abs().max(if dot < 0.0 && c == 0.0 { std::f64::consts::PI } else { 0.0 }),
                &[],
                out,
            );
        } else if turn < 0.0 {
            pie(p[v], d[v], n2, n1, -turn, &[], out);
        }
    }
    if !closed {
        if let (true, Some(i)) = (round_ends[0], first) {
            let u = dirs[i].unwrap();
            let back = [-u[0], -u[1]];
            pie(p[0], d[0], left(back).map(|x| -x), left(back), std::f64::consts::PI, cuts[0], out);
        }
        if let (true, Some(i)) = (round_ends[1], last) {
            let u = dirs[i].unwrap();
            pie(p[n - 1], d[n - 1], left(u).map(|x| -x), left(u), std::f64::consts::PI, cuts[1], out);
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

/// The pieces of rings (NonZero) that lines cut them into, in a frame about `anchor`: each an outer
/// ring and its holes. Every vertex of the rings stays.
pub fn cut(rings: &[Vec<P>], lines: &[[P; 2]], anchor: P) -> Vec<Rings> {
    let pt = |p: P| IntPoint::new(p[0] - anchor[0], p[1] - anchor[1]);
    let keep = IntOverlayOptions { preserve_input_collinear: true, preserve_output_collinear: true, ..options() };
    let size = rings.iter().map(Vec::len).sum::<usize>() + lines.len();
    let mut overlay = StringOverlay::with_options(size, keep);
    for r in rings.iter().filter(|r| r.len() >= 3) {
        overlay.add_shape_contour(&r.iter().map(|&p| pt(p)).collect::<Vec<_>>());
    }
    for &[a, b] in lines {
        overlay.add_string_line([pt(a), pt(b)]);
    }
    let Some(graph) = overlay.build_graph_view_with_solver(FillRule::NonZero, solver()) else {
        return vec![rings.to_vec()];
    };
    (graph.extract_shapes_custom(StringRule::Slice, keep).into_iter())
        .map(|shape| {
            shape.into_iter().map(|c| c.into_iter().map(|q| [q.x + anchor[0], q.y + anchor[1]]).collect()).collect()
        })
        .collect()
}

pub fn union(rings: &[Vec<P>], anchor: P) -> Rings {
    boolean(rings, &[], OverlayRule::Subject, anchor)
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

    #[test]
    fn a_tapered_bend_with_a_short_segment_strokes_without_a_step() {
        // A taper on the inner side of a bend, with two samples 39 units apart: the short segment's
        // inner mitres would turn its edge back. Its vertex is left out, so the inner edge turns by
        // the bend only (under 20 degrees at every vertex), not by a step of two right angles.
        let p =
            [[-2338, 925], [-2267, 303], [-2123, -306], [-1904, -892], [-1888, -928], [-1590, -1479], [-1227, -1988]];
        let d = [2124.0, 2163.0, 2199.8, 2106.0, 2100.0, 2006.1, 1912.3];
        let mut out = Vec::new();
        super::pieces_along(&p, &d, &[], [&[], &[]], [false, false], false, &mut out);
        let u = super::union(&out, [0, 0]);
        assert_eq!(u.len(), 1);
        let r = &u[0];
        let n = r.len();
        for i in 0..n {
            let (a, b, c) = (r[(i + n - 1) % n], r[i], r[(i + 1) % n]);
            let (u1, u2) =
                ([f64::from(b[0] - a[0]), f64::from(b[1] - a[1])], [f64::from(c[0] - b[0]), f64::from(c[1] - b[1])]);
            let turn = libm::atan2(u1[0] * u2[1] - u1[1] * u2[0], u1[0] * u2[0] + u1[1] * u2[1]).to_degrees();
            // The four corners of the square ends turn by a right angle; no other vertex may.
            let end = [p[0], p[6]].iter().any(|e| libm::hypot(f64::from(b[0] - e[0]), f64::from(b[1] - e[1])) < 2200.0);
            assert!(end || turn.abs() < 20.0, "vertex {b:?} turns {turn:.1} degrees");
        }
    }
}
