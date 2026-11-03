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

fn options() -> IntOverlayOptions {
    IntOverlayOptions {
        preserve_input_collinear: true,
        output_direction: ContourDirection::CounterClockwise,
        preserve_output_collinear: true,
        min_output_area: 0,
    }
}

/// The patches of the window [x0, y0, x1, y1] sliced by closed strings and open lines: each patch
/// its rings, ring 0 positive, holes negative. Also the number of input segments.
pub fn slice(window: [i32; 4], strings: &[Vec<P>], lines: &[[P; 2]]) -> (Vec<Vec<Vec<P>>>, usize) {
    let pt = |p: P| IntPoint::new(p[0], p[1]);
    let segments: usize = strings.iter().map(Vec::len).sum::<usize>() + lines.len() + 4;
    let mut overlay = StringOverlay::with_options(segments, options());
    let [x0, y0, x1, y1] = window;
    overlay.add_shape_contour(&[pt([x0, y0]), pt([x1, y0]), pt([x1, y1]), pt([x0, y1])]);
    for ring in strings.iter().filter(|r| r.len() >= 2) {
        let ring: Vec<IntPoint> = ring.iter().map(|&p| pt(p)).collect();
        overlay.add_string_contour(&ring);
    }
    for &[a, b] in lines {
        overlay.add_string_line([pt(a), pt(b)]);
    }
    let Some(graph) = overlay.build_graph_view_with_solver(FillRule::NonZero, solver()) else {
        return (vec![vec![vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]]], segments);
    };
    let shapes = graph.extract_shapes_custom(StringRule::Slice, options());
    let patches = shapes
        .into_iter()
        .map(|shape| shape.into_iter().map(|c| c.into_iter().map(|p| [p.x, p.y]).collect()).collect())
        .collect();
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
    /// The outgoing half-edges of each vertex, by angle (counter-clockwise in x, y).
    pub out: Vec<Vec<u32>>,
    pub patches: Vec<Vec<Vec<P>>>,
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
    pub fn new(patches: Vec<Vec<Vec<P>>>) -> Map {
        let mut verts: Vec<P> = patches.iter().flatten().flatten().copied().collect();
        verts.sort_by_key(|p| (p[1], p[0]));
        verts.dedup();
        let index = |p: &P| verts.binary_search_by_key(&(p[1], p[0]), |q| (q[1], q[0])).unwrap() as u32;
        let mut half = Vec::new();
        for (patch, rings) in patches.iter().enumerate() {
            for ring in rings {
                for (i, p) in ring.iter().enumerate() {
                    let (a, b) = (index(p), index(&ring[(i + 1) % ring.len()]));
                    if a != b {
                        half.push(Half { a, b, patch: patch as u32, twin: NONE });
                    }
                }
            }
        }
        let mut order: Vec<u32> = (0..half.len() as u32).collect();
        order.sort_by_key(|&h| (half[h as usize].a, half[h as usize].b));
        for &h in &order {
            let (a, b) = (half[h as usize].a, half[h as usize].b);
            if let Ok(at) = order.binary_search_by_key(&(b, a), |&g| (half[g as usize].a, half[g as usize].b)) {
                half[h as usize].twin = order[at];
            }
        }
        let mut out = vec![Vec::new(); verts.len()];
        for (h, e) in half.iter().enumerate() {
            out[e.a as usize].push(h as u32);
        }
        let dir = |h: u32, half: &[Half]| {
            let e = half[h as usize];
            let (a, b) = (verts[e.a as usize], verts[e.b as usize]);
            [i64::from(b[0]) - i64::from(a[0]), i64::from(b[1]) - i64::from(a[1])]
        };
        for list in &mut out {
            list.sort_by(|&x, &y| by_angle(dir(x, &half), dir(y, &half)));
        }
        Map { verts, half, out, patches }
    }

    pub fn point(&self, v: u32) -> P {
        self.verts[v as usize]
    }

    /// The group of each patch: patches of one key that share an edge, and not across a cell line
    /// unless `across`. `key` gives each patch's key.
    pub fn groups(&self, key: &[u32], across: bool) -> Vec<u32> {
        let mut root: Vec<u32> = (0..self.patches.len() as u32).collect();
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
        (0..self.patches.len() as u32).map(|i| find(&mut root, i)).collect()
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
        let list = &self.out[e.b as usize];
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
    /// of vertex indices, twice its area). Ordered by the first half-edge of each cycle.
    pub fn loops(&self, group: &[u32]) -> Vec<(u32, Vec<u32>, i128)> {
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
                cycle.push(self.half[h as usize].a);
                h = self.next(group, h);
            }
            let mut stack: Vec<u32> = Vec::new();
            for v in cycle {
                if let Some(at) = stack.iter().position(|&w| w == v) {
                    let lp: Vec<u32> = stack.split_off(at);
                    stack.push(v);
                    push_loop(&mut out, g, lp, self);
                } else {
                    stack.push(v);
                }
            }
            push_loop(&mut out, g, stack, self);
        }
        out
    }
}

fn push_loop(out: &mut Vec<(u32, Vec<u32>, i128)>, g: u32, lp: Vec<u32>, map: &Map) {
    if lp.len() >= 3 {
        let ring: Vec<P> = lp.iter().map(|&v| map.point(v)).collect();
        let a = area2(&ring);
        if a != 0 {
            out.push((g, lp, a));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_squares_that_share_an_edge_slice_the_window_into_patches_with_twins() {
        let window = [0, 0, 100, 100];
        let a = vec![[10, 10], [50, 10], [50, 50], [10, 50]];
        let b = vec![[50, 10], [90, 10], [90, 50], [50, 50]];
        let (patches, _) = slice(window, &[a, b], &[]);
        assert_eq!(patches.len(), 3);
        let total: i128 = patches.iter().flatten().map(|r| area2(r)).sum();
        assert_eq!(total, 2 * 100 * 100);
        let map = Map::new(patches);
        let inner = map.half.iter().filter(|e| e.twin != NONE).count();
        let border = map.half.iter().filter(|e| e.twin == NONE).count();
        assert_eq!(border, 4, "the window's four sides");
        assert!(inner > 0 && inner % 2 == 0);
        let key = vec![0; 3];
        let one = map.groups(&key, false);
        let loops = map.loops(&one);
        assert_eq!(loops.len(), 1, "one group, the window: {loops:?}");
        assert_eq!(loops[0].2, 2 * 100 * 100);
    }
}
