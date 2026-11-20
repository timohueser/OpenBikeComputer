//! The arch: a head wall on the frame line, with a frame round the opening and wings whose tops follow the rows of
//! the bake; the outer shell of the collar; the dark face in the opening; and the box in which the ground does not
//! draw.

use crate::mesh::{flagged, flat, outline, Builder, Corner, Station, VOID};
use crate::tube::{self, direction, inward, spring, Net};
use crate::{metres_per_unit, Section, BOX};

/// The pieces of the collar's shell of end e: per chord from the back of the head wall to `collar` metres in, its
/// start and end station across to the left of the way in, its chord's height and section.
pub fn collar(net: &Net, e: usize) -> Vec<(Station, Station, f64, Section)> {
    let end = &net.ends[e];
    let design = &net.rules.design;
    let from = design.face_m - design.recess_m;
    let mut out = Vec::new();
    for (c, forward, start) in net.walk(e, end.collar) {
        let chord = &net.chords[c];
        let len = direction(chord).1;
        if start + len <= from || start >= end.collar {
            continue;
        }
        let (k0, k1) = if forward { (0, 1) } else { (1, 0) };
        let (near, far) = (net.ring_along(c, k0, forward), net.ring_along(c, k1, forward));
        let s = if start <= from { net.frame(e, from) } else { near };
        let t = if start + len > end.collar { near.mix(&far, (end.collar - start) / len) } else { far };
        out.push((s, t, chord.height, chord.section));
    }
    out
}

/// The top of the head wall `t` metres across to the left of its axis, over the opening's floor: `face` over the
/// frame, then the rows of the wings, a value per metre out from the frame's outer edge `edge`.
fn top(end: &crate::End, edge: f64, t: f64) -> f64 {
    let (row, d) = if t > edge {
        (&end.left, t - edge)
    } else if t < -edge {
        (&end.right, -t - edge)
    } else {
        return end.face;
    };
    let at = |i: usize| if i == 0 || row.is_empty() { end.face } else { row[(i - 1).min(row.len() - 1)] };
    let j = d.floor() as usize;
    at(j) + (at(j + 1) - at(j)) * (d - j as f64)
}

/// The faces of the arch at end e: its head wall, the reveal and the frame round the opening, the dark face in it,
/// and the shell of its collar.
pub fn faces<L: Fn([f64; 3]) -> f64>(b: &mut Builder<L>, net: &Net, e: usize) {
    let end = &net.ends[e];
    let Some(look) = net.rules.looks.get(&end.look) else { return };
    let (design, bake) = (&net.rules.design, &net.rules.ladder.bake);
    let n = design.arch_segments;
    let edge = end.half + bake.shell_m;
    let sp = spring(net, end.section, end.height);
    let inner = outline(end.section, end.half, end.height, sp, n);
    let hole = outline(end.section, end.half + design.frame_m, end.height + design.frame_m, sp, n);
    let (front, ring, back) =
        (net.frame(e, -design.recess_m), net.frame(e, 0.0), net.frame(e, design.face_m - design.recess_m));
    let into = end.way_in();
    let (inward_, outward) = (flat(into), flat([-into[0], -into[1]]));
    let (wide_r, wide_l) = (-(edge + end.right.len() as f64), edge + end.left.len() as f64);
    let top = |t: f64| top(end, edge, t);
    let wall = Wall { top: &top, foot: end.foot, coping: bake.coping_m, face: look.face, cope: look.coping };
    wall.face(b, &front, &hole, (wide_r, wide_l), edge, outward);
    wall.face(b, &back, &inner, (wide_r, wide_l), edge, inward_);
    // The top over the whole wall, and its two ends.
    let tops = metres(wide_r, -edge).into_iter().chain(metres(edge, wide_l));
    let tops: Vec<f64> = tops.collect();
    for w in tops.windows(2) {
        let c = |s: &Station, t: f64| s.at([t, top(t)]);
        let v = [c(&front, w[0]), c(&front, w[1]), c(&back, w[1]), c(&back, w[0])]
            .map(|p| b.vertex(p, look.coping, crate::mesh::PLAIN));
        b.quad(v, [0.0, 0.0, 1.0]);
    }
    for t in [wide_r, wide_l] {
        let side = front.out([t.signum(), 0.0]);
        wall.band(b, [(&front, t), (&back, t)], [end.foot; 2], [top(t); 2], side);
    }
    // The reveal from the front to the frame, and the frame round the opening. Over the sill, the road of the ground
    // reaches the frame line.
    for k in 0..hole.len() - 1 {
        let v = [
            b.at(&front, hole[k], look.face),
            b.at(&front, hole[k + 1], look.face),
            b.at(&ring, hole[k + 1], look.face),
            b.at(&ring, hole[k], look.face),
        ];
        b.quad(v, front.out(inward(hole[k], hole[k + 1], sp)));
        b.plane(&ring, [hole[k], hole[k + 1], inner[k + 1], inner[k]], look.frame, outward);
    }
    // The dark face in the opening, which the viewer draws where the terrain is coarse.
    let dark = flagged(net.rules.inside.void, VOID);
    let st = net.frame(e, design.void_m);
    let centre = b.at(&st, [0.0, end.height / 2.0], dark);
    let rim: Vec<u32> = inner.iter().map(|&c| b.at(&st, c, dark)).collect();
    for k in 0..rim.len() {
        b.triangle(centre, rim[k], rim[(k + 1) % rim.len()], outward);
    }
    tube::shell(b, net, &collar(net, e), end.half, end.z + end.foot, look.shell);
}

/// Whole metres from a to b, with a and b.
fn metres(a: f64, b: f64) -> Vec<f64> {
    let mut out = vec![a];
    let mut t = a.floor() + 1.0;
    while t < b - 1e-9 {
        if t > a + 1e-9 {
            out.push(t);
        }
        t += 1.0;
    }
    if b > a {
        out.push(b);
    }
    out
}

/// A face of a head wall: its top, its foot (over the opening's floor), the depth of its coping and its colours.
struct Wall<'a> {
    top: &'a dyn Fn(f64) -> f64,
    foot: f64,
    coping: f64,
    face: u32,
    cope: u32,
}

impl Wall<'_> {
    /// The face at station `st` from `wide.0` to `wide.1` across, with the hole of an opening's outline (from its right
    /// foot over its top to its left foot): columns from the foot, or from the hole, up to the top, and the sill under
    /// the hole. The wall's top is flat out to `edge`, and per metre out past it.
    fn face<L: Fn([f64; 3]) -> f64>(
        &self,
        b: &mut Builder<L>,
        st: &Station,
        hole: &[Corner],
        wide: (f64, f64),
        edge: f64,
        out: [f64; 3],
    ) {
        let hw = hole[0][0].abs();
        let n = hole.len();
        let right: Vec<f64> = metres(wide.0, -edge).into_iter().filter(|&t| t < -hw).chain([-hw]).collect();
        let left: Vec<f64> = [hw].into_iter().chain(metres(edge, wide.1).into_iter().filter(|&t| t > hw)).collect();
        for xs in [&right, &left] {
            for w in xs.windows(2) {
                self.band(b, [(st, w[0]), (st, w[1])], [self.foot; 2], [(self.top)(w[0]), (self.top)(w[1])], out);
            }
        }
        for w in hole[1..n - 1].windows(2) {
            self.band(
                b,
                [(st, w[0][0]), (st, w[1][0])],
                [w[0][1], w[1][1]],
                [(self.top)(w[0][0]), (self.top)(w[1][0])],
                out,
            );
        }
        b.plane(st, [[-hw, self.foot], [hw, self.foot], [hw, 0.0], [-hw, 0.0]], self.face, out);
    }

    /// The quad between two columns (a station and metres across each) from their bottoms to their tops, at least
    /// their bottoms; the top `coping` in the coping's colour.
    fn band<L: Fn([f64; 3]) -> f64>(
        &self,
        b: &mut Builder<L>,
        at: [(&Station, f64); 2],
        bottom: [f64; 2],
        top: [f64; 2],
        out: [f64; 3],
    ) {
        let top = [top[0].max(bottom[0]), top[1].max(bottom[1])];
        let cut = [(top[0] - self.coping).max(bottom[0]), (top[1] - self.coping).max(bottom[1])];
        let c = |b: &mut Builder<L>, i: usize, up: f64, colour: u32| b.at(at[i].0, [at[i].1, up], colour);
        let v = [
            c(b, 0, bottom[0], self.face),
            c(b, 1, bottom[1], self.face),
            c(b, 1, cut[1], self.face),
            c(b, 0, cut[0], self.face),
        ];
        b.quad(v, out);
        let v = [
            c(b, 0, cut[0], self.cope),
            c(b, 1, cut[1], self.cope),
            c(b, 1, top[1], self.cope),
            c(b, 0, top[0], self.cope),
        ];
        b.quad(v, out);
    }
}

/// The box of end e (`BOX` words): its middle on the frame line, its unit way in, its depth (`portal_cells` and a cell's
/// diagonal of mesh cells of level 17, and the collar), its half-width to the outside of the shell, the height of its
/// roof and its floor at the frame line, and their rise per metre in. The rise is the least over the chord vertices in
/// the box, so the box's foot stays under the floor, and the roof rises until it clears the crown at each.
pub fn portal_box(net: &Net, e: usize) -> [f64; BOX] {
    let end = &net.ends[e];
    let (design, bake) = (&net.rules.design, &net.rules.ladder.bake);
    let m = metres_per_unit(end.at[1]);
    let cells = (design.portal_cells + std::f64::consts::SQRT_2) * design.mesh_texels * 32.0 * m + 1.0;
    let mut depth = cells.max(end.collar + 2.0);
    let walk = net.walk(e, depth);
    // Per chord vertex in the box: its metres in, its floor and its crown over the opening's.
    let mut places: Vec<(f64, f64, f64)> = Vec::new();
    for &(c, forward, start) in &walk {
        let chord = &net.chords[c];
        let len = direction(chord).1;
        let (near, far) = if forward { (chord.a[2], chord.b[2]) } else { (chord.b[2], chord.a[2]) };
        let s = (start + len).min(depth);
        let z = near + (far - near) * (s - start) / len;
        for (s, z) in [(start, near), (s, z)] {
            places.push((s, z - end.z, z + chord.height - end.z - end.height));
        }
    }
    // A box ends at the other opening of a shorter tube.
    if let Some(&(c, forward, start)) = walk.last() {
        if net.opening_at(c, usize::from(forward)) {
            depth = depth.min(start + direction(&net.chords[c]).1);
        }
    }
    let inside: Vec<_> = places.into_iter().filter(|p| p.0 > 1e-9 && p.0 <= depth).collect();
    let rise = inside.iter().map(|p| p.1 / p.0).fold(f64::INFINITY, f64::min);
    let rise = if rise.is_finite() { rise } else { 0.0 };
    let lift = inside.iter().map(|p| p.2 - rise * p.0).fold(0.0, f64::max);
    let into = end.way_in();
    [
        end.at[0],
        end.at[1],
        into[0],
        into[1],
        depth,
        end.half + bake.shell_m,
        end.z + end.height + bake.shell_m + lift,
        end.z + design.lift_m,
        rise,
    ]
}
