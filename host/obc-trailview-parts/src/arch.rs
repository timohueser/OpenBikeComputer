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

/// The faces of the arch at end e: its head wall, the sill, the reveal and the frame round the opening, the dark face
/// in it, and the shell of its collar. The head wall and the shell go down to one level, `foot` under the opening's
/// floor; the sill lies `lift_m` under the floor, under the road of the tube and of the ground. Faces that meet share
/// the corners of their edges.
pub fn faces<L: Fn([f64; 3]) -> f64>(b: &mut Builder<L>, net: &Net, e: usize) {
    let end = &net.ends[e];
    let Some((c, _)) = net.opening(e) else { return };
    let chord = &net.chords[c];
    let look = &net.rules.looks[&end.look];
    let (design, bake) = (&net.rules.design, &net.rules.ladder.bake);
    let (n, sill, foot) = (design.arch_segments, -design.lift_m, end.z + end.foot);
    let (half, edge) = (chord.half, chord.half + bake.shell_m);
    let sp = spring(net, chord.section, chord.height);
    let pieces = tube::floor_pieces(net, chord);
    // An outline of the opening grown by `grow`, from the sill up its side to a corner at the floor's height there,
    // where the tube's wall stands on the floor, and on over its top.
    let grown = |grow: f64| {
        let o = outline(chord.section, half + grow, chord.height + grow, sp, n);
        let (first, last) = (o[0][0], o[o.len() - 1][0]);
        let sides = |x: f64, top: f64| [[x, sill], [x, top]];
        let mut out = sides(first, pieces[0].2).to_vec();
        out.extend_from_slice(&o[1..o.len() - 1]);
        out.extend(sides(last, pieces[pieces.len() - 1].2).into_iter().rev());
        out
    };
    let (inner, hole) = (grown(0.0), grown(design.frame_m));
    let (front, ring, back) =
        (net.frame(e, -design.recess_m), net.frame(e, 0.0), net.frame(e, design.face_m - design.recess_m));
    let shell = collar(net, e);
    let (h, section) = shell.first().map_or((chord.height, chord.section), |p| (p.2, p.3));
    let into = end.way_in();
    let (inward_, outward) = (flat(into), flat([-into[0], -into[1]]));
    let wide = (-(edge + end.right.len() as f64), edge + end.left.len() as f64);
    let top = |t: f64| top(end, edge, t);
    let wall = Wall { top: &top, wide, edge, foot, coping: bake.coping_m, face: look.face, cope: look.coping };
    // The sill is split where the pieces of the floor meet the front of the tube.
    let hw = hole[0][0].abs();
    let sills: Vec<f64> =
        [-hw].into_iter().chain(pieces.iter().map(|p| p.0)).chain([pieces[pieces.len() - 1].1, hw]).collect();
    let front_top = wall.face(b, &front, &hole, &sills, outward);
    // The back's hole is the outside of the collar's shell, which goes on from it.
    let back_top = wall.face(b, &back, &tube::shell_outline(net, section, half, h, foot - back.z), &[], inward_);
    b.strip((&front, &front_top), (&back, &back_top), look.coping, [0.0, 0.0, 1.0]);
    for t in [wide.0, wide.1] {
        wall.column(
            b,
            (&front, t, &wall.side(&front, t, &[])),
            (&back, t, &wall.side(&back, t, &[])),
            front.out([t.signum(), 0.0]),
        );
    }
    // The sill from the front to the frame line, the reveal, and the frame round the opening.
    for w in sills.windows(2) {
        let line = [[w[0], sill], [w[1], sill]];
        b.strip((&front, &line), (&ring, &line), look.face, [0.0, 0.0, 1.0]);
    }
    for k in 0..hole.len() - 1 {
        b.strip(
            (&front, &hole[k..k + 2]),
            (&ring, &hole[k..k + 2]),
            look.face,
            front.out(inward(hole[k], hole[k + 1], sp)),
        );
        b.strip((&ring, &hole[k..k + 2]), (&ring, &inner[k..k + 2]), look.frame, outward);
    }
    // The dark face in the opening, which the viewer draws where the terrain is coarse.
    let dark = flagged(net.rules.inside.void, VOID);
    let st = net.frame(e, design.void_m);
    let centre = b.at(&st, [0.0, chord.height / 2.0], dark);
    let rim: Vec<u32> = inner.iter().map(|&c| b.at(&st, c, dark)).collect();
    for k in 0..rim.len() {
        b.triangle(centre, rim[k], rim[(k + 1) % rim.len()], outward);
    }
    tube::shell(b, net, &shell, half, foot, look.shell);
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

/// A face of a head wall: its top over the opening's floor, flat out to `edge` across and per metre out past it, the
/// metres across of its two ends, the height of its foot, the depth of its coping and its colours.
struct Wall<'a> {
    top: &'a dyn Fn(f64) -> f64,
    wide: (f64, f64),
    edge: f64,
    foot: f64,
    coping: f64,
    face: u32,
    cope: u32,
}

/// The heights of the corners of a column of a wall, from its bottom up, and the place of the coping's foot in them.
type Side = (Vec<f64>, usize);

impl Wall<'_> {
    /// The side of a column at `x` across at station `st`: the corners from the foot, or from `bottoms` where the
    /// column stands on a hole (their heights up), to the coping's foot and the top, both at least the highest bottom.
    fn side(&self, st: &Station, x: f64, bottoms: &[f64]) -> Side {
        let mut ups: Vec<f64> = if bottoms.is_empty() { vec![self.foot - st.z] } else { bottoms.to_vec() };
        let low = ups[ups.len() - 1];
        let top = (self.top)(x).max(low);
        let cut = (top - self.coping).max(low);
        ups.extend([cut, top]);
        ups.dedup();
        let at = ups.iter().position(|&u| u == cut).unwrap();
        (ups, at)
    }

    /// The faces between two sides of a column: the wall up to the coping's foot, and the coping.
    fn column<L: Fn([f64; 3]) -> f64>(
        &self,
        b: &mut Builder<L>,
        (s, x0, a): (&Station, f64, &Side),
        (t, x1, z): (&Station, f64, &Side),
        out: [f64; 3],
    ) {
        let corners = |x: f64, ups: &[f64]| ups.iter().map(|&u| [x, u]).collect::<Vec<Corner>>();
        b.strip((s, &corners(x0, &a.0[..=a.1])), (t, &corners(x1, &z.0[..=z.1])), self.face, out);
        b.strip((s, &corners(x0, &a.0[a.1..])), (t, &corners(x1, &z.0[z.1..])), self.cope, out);
    }

    /// The face at station `st` from end to end, round the hole of an outline (from the bottom of its right side up
    /// and over its top to the bottom of its left side): columns from the foot up beside the hole, columns from the
    /// hole up over it, and under the hole the sill, split at `sills`. Gives the corners of the top.
    fn face<L: Fn([f64; 3]) -> f64>(
        &self,
        b: &mut Builder<L>,
        st: &Station,
        hole: &[Corner],
        sills: &[f64],
        out: [f64; 3],
    ) -> Vec<Corner> {
        let (n, hw, foot, (wide, edge)) = (hole.len(), hole[0][0].abs(), self.foot - st.z, (self.wide, self.edge));
        // The side of the hole up to where its top starts.
        let rise = hole.iter().position(|p| p[0] != hole[0][0]).unwrap() - 1;
        let jamb: Vec<f64> = [foot].into_iter().chain(hole[..=rise].iter().map(|p| p[1])).collect();
        let mut columns: Vec<(f64, Side)> =
            (metres(wide.0, -edge).into_iter().filter(|&t| t < -hw)).map(|t| (t, self.side(st, t, &[]))).collect();
        columns.push((-hw, self.side(st, -hw, &jamb)));
        let over: Vec<(f64, Side)> =
            hole[rise..n - rise].iter().map(|p| (p[0], self.side(st, p[0], &[p[1]]))).collect();
        let mut left = vec![(hw, self.side(st, hw, &jamb))];
        left.extend((metres(edge, wide.1).into_iter().filter(|&t| t > hw)).map(|t| (t, self.side(st, t, &[]))));
        for list in [&columns, &over, &left] {
            for w in list.windows(2) {
                self.column(b, (st, w[0].0, &w[0].1), (st, w[1].0, &w[1].1), out);
            }
        }
        if hole[0][1] > foot {
            for w in sills.windows(2) {
                let (low, high) = ([[w[0], foot], [w[1], foot]], [[w[0], hole[0][1]], [w[1], hole[0][1]]]);
                b.strip((st, &low), (st, &high), self.face, out);
            }
        }
        let tops = columns.iter().chain(&over[1..over.len() - 1]).chain(&left);
        tops.map(|(x, (ups, _))| [*x, ups[ups.len() - 1]]).collect()
    }
}

/// The box of end e (`BOX` words): its middle on the front of the head wall, as the footprint, its unit way in, its
/// depth (`portal_cells` and a cell's diagonal of mesh cells of level 17, and the collar, from the frame line), its
/// half-width to the outside of the shell, the height of its roof and its floor at its middle, and their rise per
/// metre in. The rise is the least over the chord vertices in
/// the box, so the box's foot stays under the floor, and the roof rises until it clears the crown at each.
pub fn portal_box(net: &Net, e: usize) -> [f64; BOX] {
    let end = &net.ends[e];
    let (design, bake) = (&net.rules.design, &net.rules.ladder.bake);
    let m = metres_per_unit(end.at[1]);
    let cells = (design.portal_cells + std::f64::consts::SQRT_2) * design.mesh_texels * 32.0 * m + 1.0;
    let mut depth = cells.max(end.collar + 2.0);
    let walk = net.walk(e, depth);
    // Per chord vertex in the box, and the opening: its metres in from the front of the head wall, its floor and
    // its crown.
    let front = net.frame(e, -design.recess_m);
    let mut places = vec![(design.recess_m, end.z, end.z + end.height)];
    for &(c, forward, start) in &walk {
        let chord = &net.chords[c];
        let len = direction(chord).1;
        if len <= 0.0 {
            continue;
        }
        let (near, far) = if forward { (chord.a[2], chord.b[2]) } else { (chord.b[2], chord.a[2]) };
        let s = (start + len).min(depth);
        let z = near + (far - near) * (s - start) / len;
        for (s, z) in [(start, near), (s, z)] {
            places.push((s + design.recess_m, z, z + chord.height));
        }
    }
    // A box ends at the other opening of a shorter tube.
    if let Some(&(c, forward, start)) = walk.last() {
        if net.opening_at(c, usize::from(forward)) {
            depth = depth.min(start + direction(&net.chords[c]).1);
        }
    }
    let depth = depth + design.recess_m;
    let inside: Vec<_> = places.into_iter().filter(|p| p.0 <= depth).collect();
    let rise = inside.iter().map(|p| (p.1 - front.z) / p.0).fold(f64::INFINITY, f64::min);
    let crown = front.z + end.height;
    let lift = inside.iter().map(|p| p.2 - crown - rise * p.0).fold(0.0, f64::max);
    let into = end.way_in();
    [
        front.x,
        front.y,
        into[0],
        into[1],
        depth,
        end.half + bake.shell_m,
        crown + bake.shell_m + lift,
        front.z + design.lift_m,
        rise,
    ]
}
