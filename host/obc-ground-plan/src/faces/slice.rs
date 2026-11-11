//! One integer slice of the window by string contours, and the planar map of its patches: shared
//! vertices, half-edges with their twins, and the boundary cycles of any grouping of the patches.

use std::cmp::Ordering;

use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay::{ContourDirection, IntOverlayOptions};
use i_overlay::core::solver::{Precision, Solver, Strategy};
use i_overlay::i_float::int::point::IntPoint;
use i_overlay::string::overlay::StringOverlay;
use i_overlay::string::rule::StringRule;

use super::geom::{area2, P};
use crate::frame::CELL_UNITS;

pub const NONE: u32 = u32::MAX;

/// One solver for every slice: one path at every segment count, a snap radius of one unit, no threads.
fn solver() -> Solver {
    Solver { strategy: Strategy::Frag, precision: Precision::ABSOLUTE, multithreading: None }
}

fn options() -> IntOverlayOptions<u64> {
    IntOverlayOptions {
        preserve_input_collinear: true,
        output_direction: ContourDirection::CounterClockwise,
        preserve_output_collinear: true,
        min_output_area: 0,
        ogc: false,
    }
}

/// The slice works in tiles of this many frame units: a cell is 8 × 8 tiles, each sliced by the
/// segments that reach it. A pass of the overlay's solver then reads one tile, so a place that needs
/// many passes costs them in its tile only. The faces' rings and seams have a vertex at each tile line
/// they cross (`geom::grid_points`), so no tile rounds a crossing of its own side; the snap rounding of
/// all patches together joins the tiles.
pub const TILE: i64 = CELL_UNITS / 8;

/// Whether coordinate x lies on a cell line.
fn line(x: i32) -> bool {
    i64::from(x).rem_euclid(CELL_UNITS) == 0
}

/// The patches of the window [x0, y0, x1, y1] sliced by closed strings and open lines: each patch
/// its rings, ring 0 positive, holes negative. Also the number of input segments.
pub fn slice(window: [i32; 4], strings: &[Vec<P>], lines: &[[P; 2]]) -> (Vec<Vec<Vec<P>>>, usize) {
    let pt = |p: P| IntPoint::new(p[0], p[1]);
    let [x0, y0, x1, y1] = window;
    // The tile bounds along each axis.
    let bounds = |lo: i32, hi: i32| {
        let mut b = vec![lo];
        let mut v = (i64::from(lo).div_euclid(TILE) + 1) * TILE;
        while v < i64::from(hi) {
            b.push(v as i32);
            v += TILE;
        }
        b.push(hi);
        b
    };
    let (bx, by) = (bounds(x0, x1), bounds(y0, y1));
    let cols = bx.len() - 1;
    // Each segment goes to every tile whose closed square its box meets.
    let range = |b: &[i32], lo: i32, hi: i32| -> std::ops::Range<usize> {
        let first = b.partition_point(|&x| x < lo).saturating_sub(1);
        let last = b.partition_point(|&x| x <= hi).min(b.len() - 1);
        first..last.max(first)
    };
    let mut tiles: Vec<Vec<[P; 2]>> = vec![Vec::new(); cols * (by.len() - 1)];
    let rings =
        strings.iter().filter(|r| r.len() >= 2).flat_map(|r| (0..r.len()).map(move |i| [r[i], r[(i + 1) % r.len()]]));
    let mut segments = 4;
    for [a, b] in rings.chain(lines.iter().copied()) {
        segments += 1;
        let (xs, ys) = (range(&bx, a[0].min(b[0]), a[0].max(b[0])), range(&by, a[1].min(b[1]), a[1].max(b[1])));
        for j in ys {
            for i in xs.clone() {
                tiles[j * cols + i].push([a, b]);
            }
        }
    }
    let mut patches = Vec::new();
    for (t, segs) in tiles.iter().enumerate() {
        let (i, j) = (t % cols, t / cols);
        let square = [[bx[i], by[j]], [bx[i + 1], by[j]], [bx[i + 1], by[j + 1]], [bx[i], by[j + 1]]];
        if segs.is_empty() {
            patches.push(vec![square.to_vec()]);
            continue;
        }
        let mut overlay = StringOverlay::with_options(segs.len() + 4, options());
        overlay.add_shape_contour(&square.map(pt));
        for &[a, b] in segs {
            overlay.add_string_line([pt(a), pt(b)]);
        }
        let Some(graph) = overlay.build_graph_view_with_solver(FillRule::NonZero, solver()) else {
            crate::probe::hit(crate::probe::Cap::SliceFallback);
            patches.push(vec![square.to_vec()]);
            continue;
        };
        let shapes = graph.extract_shapes_custom(StringRule::Slice, options());
        patches.extend(
            shapes
                .into_iter()
                .map(|shape| shape.into_iter().map(|c| c.into_iter().map(|p| [p.x, p.y]).collect()).collect()),
        );
    }
    (patches, segments)
}

/// Whether segment ab lies on one cell line.
pub fn on_cell_line(a: P, b: P) -> bool {
    let line = |v: i32| i64::from(v).rem_euclid(CELL_UNITS) == 0;
    (a[0] == b[0] && line(a[0])) || (a[1] == b[1] && line(a[1]))
}

#[derive(Clone, Copy, Debug)]
pub struct Half {
    pub a: u32,
    pub b: u32,
    pub patch: u32,
    pub twin: u32,
}

/// The planar map of a slice.
pub struct Map {
    pub verts: Vec<P>,
    pub half: Vec<Half>,
    /// The outgoing half-edges of vertex v: `out[start[v]..start[v + 1]]`, by angle (counter-clockwise
    /// in x, y).
    start: Vec<u32>,
    out: Vec<u32>,
    patches: usize,
}

/// The plan's precision in frame units (about 6 mm): every vertex of a plan lies on the global grid
/// of this step, and an edge that passes through the grid square of a vertex runs through it.
pub const PRECISION: i32 = 4;

/// A coordinate on the grid of `PRECISION`: the nearest grid line, but one step inside where that
/// line is a cell line and the coordinate is not on it, so a vertex that only one cell holds never
/// lands on the line two cells share.
fn grid(v: i32) -> i32 {
    let h = PRECISION / 2;
    let g = (v + h).div_euclid(PRECISION) * PRECISION;
    if line(g) && !line(v) {
        if v > g {
            g + PRECISION
        } else {
            g - PRECISION
        }
    } else {
        g
    }
}

/// Whether p lies on a cell line.
fn border(p: P) -> bool {
    line(p[0]) || line(p[1])
}

fn on_grid(p: P) -> P {
    [grid(p[0]), grid(p[1])]
}

/// The open square of the coordinates that round to grid coordinate g along one axis: half a step
/// each way, but up to the cell line where the next grid line is one, as `grid` rounds them.
fn square(g: i32) -> (i32, i32) {
    let r = PRECISION / 2;
    let lo = if line(g - PRECISION) { g - PRECISION } else { g - r };
    let hi = if line(g + PRECISION) { g + PRECISION } else { g + r };
    (lo, hi)
}

/// Whether segment ab meets the open square of the points that round to h.
fn through(a: P, b: P, h: P) -> bool {
    let ((lx, hx), (ly, hy)) = (square(h[0]), square(h[1]));
    let (lo, hi) = ([lx, ly], [hx, hy]);
    if a[0].max(b[0]) <= lo[0] || a[0].min(b[0]) >= hi[0] || a[1].max(b[1]) <= lo[1] || a[1].min(b[1]) >= hi[1] {
        return false;
    }
    let side =
        |c: P| i128::from(b[0] - a[0]) * i128::from(c[1] - a[1]) - i128::from(b[1] - a[1]) * i128::from(c[0] - a[0]);
    let s = [side(lo), side([hi[0], lo[1]]), side(hi), side([lo[0], hi[1]])];
    !(s.iter().all(|&x| x >= 0) || s.iter().all(|&x| x <= 0))
}

/// Patches on the grid: the grid points by (y, x), and per patch its directed segments as pairs of
/// their indices, its area on their left.
pub struct Snapped {
    pub verts: Vec<P>,
    pub patches: Vec<Vec<[u32; 2]>>,
}

/// The hot squares that segment ab passes besides those of its ends: the grid points (with their
/// index) whose squares it meets, in order along it. A square reaches at most a whole step from its
/// grid point, so the candidates lie in a slab about the segment along its shorter extent: `verts`
/// is ordered by (y, x) and `by_x` by (x, y).
fn passed(a: P, b: P, verts: &[P], by_x: &[u32], out: &mut Vec<(P, u32)>) {
    out.clear();
    let (ga, gb) = (on_grid(a), on_grid(b));
    let r = PRECISION;
    let (x0, x1, y0, y1) = (a[0].min(b[0]) - r, a[0].max(b[0]) + r, a[1].min(b[1]) - r, a[1].max(b[1]) + r);
    let mut test = |h: P, i: u32| {
        // The square of a vertex on a cell line holds only the edges on that line: the next cell holds
        // the line's vertices but not this cell's edges beside it.
        if h != ga && h != gb && (!border(h) || on_cell_line(a, b)) && through(a, b, h) {
            out.push((h, i));
        }
    };
    if x1 - x0 <= y1 - y0 {
        let from = by_x.partition_point(|&i| verts[i as usize][0] < x0);
        for &i in &by_x[from..] {
            let h = verts[i as usize];
            if h[0] > x1 {
                break;
            }
            if (y0..=y1).contains(&h[1]) {
                test(h, i);
            }
        }
    } else {
        let from = verts.partition_point(|h| h[1] < y0);
        for (i, &h) in verts.iter().enumerate().skip(from) {
            if h[1] > y1 {
                break;
            }
            if (x0..=x1).contains(&h[0]) {
                test(h, i as u32);
            }
        }
    }
    let d = [i128::from(b[0] - a[0]), i128::from(b[1] - a[1])];
    out.sort_unstable_by_key(|&(h, _)| (d[0] * i128::from(h[0] - a[0]) + d[1] * i128::from(h[1] - a[1]), h));
    out.dedup();
}

/// Snap rounding of the patches to the plan's precision: every vertex moves to its grid point, and
/// every edge runs through the grid point of each vertex whose square it passes (a hot square), in
/// order along it. Two patches that share an edge share its rounded path. Per patch, a segment and
/// its reverse cancel: a part of a patch thinner than the precision collapses, and a vertex or a step
/// closer than it merges.
pub fn snap_round(patches: &[Vec<Vec<P>>]) -> Snapped {
    let mut verts: Vec<P> = patches.iter().flatten().flatten().map(|&p| on_grid(p)).collect();
    verts.sort_unstable_by_key(|p| (p[1], p[0]));
    verts.dedup();
    let mut by_x: Vec<u32> = (0..verts.len() as u32).collect();
    by_x.sort_unstable_by_key(|&i| verts[i as usize]);
    let index = |p: P| verts.binary_search_by_key(&(p[1], p[0]), |q| (q[1], q[0])).unwrap() as u32;
    // Every directed segment of every patch by its edge: the edge from its lesser end, the patch, and
    // whether the segment runs that way.
    let mut segs: Vec<([P; 2], u32, bool)> = Vec::new();
    for (k, rings) in patches.iter().enumerate() {
        for ring in rings {
            for i in 0..ring.len() {
                let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
                if a != b {
                    segs.push(if a < b { ([a, b], k as u32, true) } else { ([b, a], k as u32, false) });
                }
            }
        }
    }
    segs.sort_unstable();
    // Per patch, its rounded segments by edge key (lesser index high) with +1 along the key, −1 against.
    let mut keyed: Vec<Vec<(u64, i32)>> = vec![Vec::new(); patches.len()];
    let (mut via, mut path) = (Vec::new(), Vec::new());
    for group in segs.chunk_by(|x, y| x.0 == y.0) {
        let [a, b] = group[0].0;
        passed(a, b, &verts, &by_x, &mut via);
        path.clear();
        path.push(index(on_grid(a)));
        path.extend(via.iter().map(|&(_, i)| i));
        path.push(index(on_grid(b)));
        for &(_, k, fwd) in group {
            for w in path.windows(2).filter(|w| w[0] != w[1]) {
                let (u, v) = if fwd { (w[0], w[1]) } else { (w[1], w[0]) };
                keyed[k as usize].push(if u < v {
                    ((u64::from(u) << 32) | u64::from(v), 1)
                } else {
                    ((u64::from(v) << 32) | u64::from(u), -1)
                });
            }
        }
    }
    let patches = keyed
        .into_iter()
        .map(|mut keyed| {
            keyed.sort_unstable();
            let mut out = Vec::new();
            for group in keyed.chunk_by(|x, y| x.0 == y.0) {
                let n: i32 = group.iter().map(|x| x.1).sum();
                let (u, v) = ((group[0].0 >> 32) as u32, group[0].0 as u32);
                for _ in 0..n.abs() {
                    out.push(if n > 0 { [u, v] } else { [v, u] });
                }
            }
            out
        })
        .collect();
    Snapped { verts, patches }
}

/// The order of directions by angle from +x toward +y, exact.
fn by_angle(u: [i64; 2], v: [i64; 2]) -> Ordering {
    let half = |d: [i64; 2]| u8::from(!(d[1] > 0 || (d[1] == 0 && d[0] > 0)));
    half(u).cmp(&half(v)).then_with(|| {
        let c = i128::from(u[0]) * i128::from(v[1]) - i128::from(u[1]) * i128::from(v[0]);
        0.cmp(&c)
    })
}

impl Map {
    /// The map of the patches' rings as they are, not rounded.
    #[cfg(test)]
    pub fn exact(patches: Vec<Vec<Vec<P>>>) -> Map {
        let mut verts: Vec<P> = patches.iter().flatten().flatten().copied().collect();
        verts.sort_unstable_by_key(|p| (p[1], p[0]));
        verts.dedup();
        let index = |p: P| verts.binary_search_by_key(&(p[1], p[0]), |q| (q[1], q[0])).unwrap() as u32;
        let patches = (patches.iter())
            .map(|rings| {
                (rings.iter())
                    .flat_map(|ring| (0..ring.len()).map(move |i| [ring[i], ring[(i + 1) % ring.len()]]))
                    .map(|[a, b]| [index(a), index(b)])
                    .collect()
            })
            .collect();
        Map::new(Snapped { verts, patches })
    }

    /// The map of snapped patches.
    pub fn new(snapped: Snapped) -> Map {
        let Snapped { verts, patches } = snapped;
        let mut half = Vec::new();
        for (patch, segments) in patches.iter().enumerate() {
            for &[a, b] in segments {
                if a != b {
                    half.push(Half { a, b, patch: patch as u32, twin: NONE });
                }
            }
        }
        let key = |e: &Half| (u64::from(e.a) << 32) | u64::from(e.b);
        let mut order: Vec<(u64, u32)> = half.iter().enumerate().map(|(h, e)| (key(e), h as u32)).collect();
        order.sort_unstable();
        for &(k, h) in &order {
            let back = k.rotate_left(32);
            if let Ok(at) = order.binary_search_by_key(&back, |x| x.0) {
                half[h as usize].twin = order[at].1;
            }
        }
        // The outgoing half-edges of each vertex: the runs of `order` by their first vertex, by angle.
        let mut start = vec![0u32; verts.len() + 1];
        for e in &half {
            start[e.a as usize + 1] += 1;
        }
        for v in 0..verts.len() {
            start[v + 1] += start[v];
        }
        let mut out: Vec<u32> = order.iter().map(|x| x.1).collect();
        let dir = |h: u32| {
            let e = half[h as usize];
            let (a, b) = (verts[e.a as usize], verts[e.b as usize]);
            [i64::from(b[0]) - i64::from(a[0]), i64::from(b[1]) - i64::from(a[1])]
        };
        for v in 0..verts.len() {
            out[start[v] as usize..start[v + 1] as usize].sort_by(|&x, &y| by_angle(dir(x), dir(y)));
        }
        Map { verts, half, start, out, patches: patches.len() }
    }

    pub fn point(&self, v: u32) -> P {
        self.verts[v as usize]
    }

    /// The group of each patch: patches of one key that share an edge, and not across a cell line
    /// unless `across`. `key` gives each patch's key.
    pub fn groups(&self, key: &[u32], across: bool) -> Vec<u32> {
        let mut root: Vec<u32> = (0..self.patches as u32).collect();
        fn find(root: &mut [u32], i: u32) -> u32 {
            let mut i = i;
            while root[i as usize] != i {
                root[i as usize] = root[root[i as usize] as usize];
                i = root[i as usize];
            }
            i
        }
        for e in &self.half {
            if e.twin == NONE {
                continue;
            }
            let other = self.half[e.twin as usize].patch;
            if key[e.patch as usize] != key[other as usize] {
                continue;
            }
            if !across && on_cell_line(self.point(e.a), self.point(e.b)) {
                continue;
            }
            let (x, y) = (find(&mut root, e.patch), find(&mut root, other));
            root[x.max(y) as usize] = x.min(y);
        }
        (0..self.patches as u32).map(|i| find(&mut root, i)).collect()
    }

    /// The group across half-edge h, or NONE past the window.
    pub fn across(&self, group: &[u32], h: u32) -> u32 {
        let t = self.half[h as usize].twin;
        if t == NONE {
            NONE
        } else {
            group[self.half[t as usize].patch as usize]
        }
    }

    /// Whether h bounds its group: the twin lies in another group or past the window.
    pub fn bounds(&self, group: &[u32], h: u32) -> bool {
        self.across(group, h) != group[self.half[h as usize].patch as usize]
    }

    /// The next boundary half-edge of h's group after h: the first outgoing edge clockwise from h's
    /// reverse at its end that bounds the same group.
    pub fn next(&self, group: &[u32], h: u32) -> u32 {
        let e = self.half[h as usize];
        let g = group[e.patch as usize];
        let (a, b) = (self.point(e.a), self.point(e.b));
        let back = [i64::from(a[0]) - i64::from(b[0]), i64::from(a[1]) - i64::from(b[1])];
        let list = &self.out[self.start[e.b as usize] as usize..self.start[e.b as usize + 1] as usize];
        let dir = |x: u32| {
            let q = self.point(self.half[x as usize].b);
            [i64::from(q[0]) - i64::from(b[0]), i64::from(q[1]) - i64::from(b[1])]
        };
        // The last edge before `back` by angle, then on clockwise.
        let start = list.partition_point(|&x| by_angle(dir(x), back) == Ordering::Less);
        for k in 1..=list.len() {
            let x = list[(start + list.len() - k) % list.len()];
            if group[self.half[x as usize].patch as usize] == g && self.bounds(group, x) {
                return x;
            }
        }
        unreachable!("a boundary half-edge has a successor in its group")
    }

    /// The boundary cycles of each group, split at repeated vertices into simple loops: (group, loop
    /// of vertex indices, twice its area, the half-edge that leaves each vertex). Ordered by the
    /// first half-edge of each cycle.
    pub fn loops(&self, group: &[u32]) -> Vec<Loop> {
        let mut seen = vec![false; self.half.len()];
        let mut out = Vec::new();
        for h0 in 0..self.half.len() as u32 {
            if seen[h0 as usize] || !self.bounds(group, h0) {
                continue;
            }
            let g = group[self.half[h0 as usize].patch as usize];
            let mut cycle = Vec::new();
            let mut h = h0;
            while !seen[h as usize] {
                seen[h as usize] = true;
                cycle.push((self.half[h as usize].a, h));
                h = self.next(group, h);
            }
            let mut stack: Vec<(u32, u32)> = Vec::new();
            for (v, h) in cycle {
                if let Some(at) = stack.iter().position(|&(w, _)| w == v) {
                    let lp = stack.split_off(at);
                    stack.push((v, h));
                    push_loop(&mut out, g, lp, self);
                } else {
                    stack.push((v, h));
                }
            }
            push_loop(&mut out, g, stack, self);
        }
        out
    }
}

/// A boundary loop of a group: the group, its vertices, twice its area, and the half-edge that
/// leaves each vertex.
pub type Loop = (u32, Vec<u32>, i128, Vec<u32>);

fn push_loop(out: &mut Vec<Loop>, g: u32, lp: Vec<(u32, u32)>, map: &Map) {
    if lp.len() >= 3 {
        let ring: Vec<P> = lp.iter().map(|&(v, _)| map.point(v)).collect();
        let a = area2(&ring);
        if a != 0 {
            out.push((g, lp.iter().map(|x| x.0).collect(), a, lp.iter().map(|x| x.1).collect()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::faces::geom::cross;

    const C: i32 = CELL_UNITS as i32;

    /// The snapped patches as point segments.
    fn snapped(patches: &[Vec<Vec<P>>]) -> Vec<Vec<[P; 2]>> {
        let s = snap_round(patches);
        let at = |i: u32| s.verts[i as usize];
        s.patches.iter().map(|segs| segs.iter().map(|&[a, b]| [at(a), at(b)]).collect()).collect()
    }

    /// The snapped patches tile the window: every vertex on the grid, every segment off the window's
    /// border has its reverse in another patch, no two segments cross, no patch has a negative area,
    /// and the areas sum to the window's.
    fn assert_partition(window: [i32; 4], snapped: &[Vec<[P; 2]>]) {
        let [x0, y0, x1, y1] = window;
        let on_border = |[a, b]: [P; 2]| {
            (a[0] == b[0] && (a[0] == x0 || a[0] == x1)) || (a[1] == b[1] && (a[1] == y0 || a[1] == y1))
        };
        let mut all: Vec<([P; 2], usize)> = Vec::new();
        let mut total = 0i128;
        for (k, segs) in snapped.iter().enumerate() {
            let area: i128 = segs.iter().map(|&[a, b]| cross([0, 0], a, b)).sum();
            assert!(area >= 0, "patch {k} has a negative area");
            total += area;
            for &s in segs {
                for p in s {
                    assert_eq!(on_grid(p), p, "vertex {p:?} is off the grid");
                }
                assert!(!segs.contains(&[s[1], s[0]]), "patch {k} keeps a segment and its reverse: {s:?}");
                all.push((s, k));
            }
        }
        assert_eq!(total, 2 * i128::from(x1 - x0) * i128::from(y1 - y0), "the areas sum to the window's");
        for &(s, k) in &all {
            if !on_border(s) {
                let twin = all.iter().any(|&(t, j)| t == [s[1], s[0]] && j != k);
                assert!(twin, "segment {s:?} of patch {k} has no twin");
            }
        }
        for (i, &([a, b], _)) in all.iter().enumerate() {
            for &([c, d], _) in &all[i + 1..] {
                let (d1, d2, d3, d4) = (cross(a, b, c), cross(a, b, d), cross(c, d, a), cross(c, d, b));
                assert!(
                    d1.signum() * d2.signum() >= 0 || d3.signum() * d4.signum() >= 0,
                    "{a:?}-{b:?} crosses {c:?}-{d:?}"
                );
            }
        }
    }

    /// Thin triangles about the corner (C, C) of a cell, with vertices 1 to 7 units off its lines.
    fn near_lines(seed: u64, n: usize) -> Vec<Vec<P>> {
        let mut s = seed;
        let mut next = |m: i32| {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((s >> 33) % m as u64) as i32
        };
        (0..n)
            .map(|_| {
                let a = [C - 1 - next(7), C - 40 - next(200)];
                let b = [C - 40 - next(200), C - 1 - next(7)];
                let c = [C - 1 - next(9), C - 1 - next(9)];
                vec![a, b, c]
            })
            .collect()
    }

    #[test]
    fn snap_rounding_keeps_a_partition_beside_the_cell_lines() {
        let window = [C - 400, C - 400, C, C];
        for seed in 0..40 {
            let rings = near_lines(seed, 6);
            // A line that runs within two units of the cell line, ending on it.
            let lines = [[[C - 3, C - 400], [C, C - 1]]];
            let (patches, _) = slice(window, &rings, &lines);
            assert_partition(window, &snapped(&patches));
        }
    }

    #[test]
    fn the_tiles_of_a_slice_join_into_one_partition() {
        // Thin triangles across the corner where four tiles meet, inside one cell.
        let t = TILE as i32;
        let window = [t - 400, t - 400, t + 400, t + 400];
        for seed in 0..20 {
            let rings: Vec<Vec<P>> = near_lines(seed, 6)
                .into_iter()
                .map(|r| r.into_iter().map(|p| [p[0] - C + t + 150, p[1] - C + t + 150]).collect())
                .collect();
            let (patches, _) = slice(window, &rings, &[[[t - 397, t - 300], [t + 300, t + 3]]]);
            assert_partition(window, &snapped(&patches));
        }
    }

    #[test]
    fn a_vertex_beside_a_cell_line_rounds_away_from_it_without_crossing_a_passing_edge() {
        // v lies right of the line from (C-3, 0) to (C, 400); its grid point lies left of where the
        // line passes, so the line must run through v's grid point.
        let window = [C - 400, 0, C, 400];
        let v = [C - 1, 199];
        let (patches, _) = slice(window, &[vec![v, [C, 150], [C, 250]]], &[[[C - 3, 0], [C, 400]]]);
        assert_partition(window, &snapped(&patches));
    }

    #[test]
    fn a_part_thinner_than_the_precision_collapses_and_its_segments_cancel() {
        // A square with a needle 3 units wide: both sides of the needle round to one grid line.
        let window = [0, 0, 400, 400];
        let ring = vec![[100, 100], [200, 100], [200, 198], [300, 198], [300, 201], [200, 201], [200, 300], [100, 300]];
        let (patches, _) = slice(window, &[ring], &[]);
        let snapped = snapped(&patches);
        assert_partition(window, &snapped);
        assert!(snapped.iter().flatten().flatten().all(|p| p[0] <= 200 || p[0] == 400), "the needle is gone");
    }

    #[test]
    fn two_cells_round_their_shared_line_alike() {
        // One shape over the line x = C, with its vertices on the line (as the build inserts them)
        // and others 1 to 3 units off it on both sides.
        let ring = vec![
            [C - 90, 103],
            [C - 1, 101],
            [C, 99],
            [C + 2, 97],
            [C + 70, 141],
            [C + 3, 207],
            [C, 209],
            [C - 2, 211],
            [C - 60, 230],
        ];
        let line = |snapped: &[Vec<[P; 2]>]| {
            let mut on: Vec<[P; 2]> = (snapped.iter().flatten())
                .filter(|[a, b]| a[0] == C && b[0] == C)
                .map(|&[a, b]| if a < b { [a, b] } else { [b, a] })
                .collect();
            on.sort_unstable();
            on.dedup();
            on
        };
        let mut sides = Vec::new();
        for window in [[C - 400, 0, C, 400], [C, 0, C + 400, 400]] {
            let snapped = snapped(&slice(window, std::slice::from_ref(&ring), &[]).0);
            assert_partition(window, &snapped);
            sides.push(line(&snapped));
        }
        assert_eq!(sides[0], sides[1]);
    }

    #[test]
    fn two_squares_that_share_an_edge_slice_the_window_into_patches_with_twins() {
        let window = [0, 0, 100, 100];
        let a = vec![[10, 10], [50, 10], [50, 50], [10, 50]];
        let b = vec![[50, 10], [90, 10], [90, 50], [50, 50]];
        let (patches, _) = slice(window, &[a, b], &[]);
        assert_eq!(patches.len(), 3);
        let total: i128 = patches.iter().flatten().map(|r| area2(r)).sum();
        assert_eq!(total, 2 * 100 * 100);
        let map = Map::exact(patches);
        let inner = map.half.iter().filter(|e| e.twin != NONE).count();
        let border = map.half.iter().filter(|e| e.twin == NONE).count();
        assert_eq!(border, 4, "the window's four sides");
        assert!(inner > 0 && inner % 2 == 0);
        let key = vec![0; 3];
        let one = map.groups(&key, false);
        let loops = map.loops(&one);
        assert_eq!(loops.len(), 1, "one group, the window: {loops:?}");
        assert_eq!(loops[0].2, 2 * 100 * 100);
        assert_eq!(loops[0].3.len(), loops[0].1.len());
    }
}
