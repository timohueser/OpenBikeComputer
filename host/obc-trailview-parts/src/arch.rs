//! The head of a part at its opening. An arch: a head wall on the frame line, with a frame round the opening and wings
//! whose tops follow the rows of the bake; the two openings of a pair have one. A rim: the head wall of an opening in
//! the rock and of a free tube end hugs the opening. Then the outer shell of the part and the dark face in the opening.
//! A free end covered at its opening ends in the hill, closed by a dark face, with no head.

use crate::boxes;
use crate::mesh::{flagged, flat, outline, Builder, Clip, Corner, Station, VOID};
use crate::tube::{self, cut_face, direction, inward, spring, steps, top_at, Net};
use crate::{Part, Section};

/// A piece of the shell: its start and end station across to the left of the way in, its chord's height and section,
/// and the chord and its vertex at the end.
pub type Piece = (Station, Station, f64, Section, usize, usize);

/// The pieces of the shell of end e: per chord from the back of the head wall, or from the frame line of a closed end,
/// to `to` metres in.
pub fn collar(net: &Net, e: usize, to: f64) -> Vec<Piece> {
    let design = &net.rules.design;
    let from = if net.closed(e) { 0.0 } else { design.face_m - design.recess_m };
    let mut out = Vec::new();
    for (c, forward, start) in net.walk(e, to) {
        let chord = &net.chords[c];
        let len = direction(chord).1;
        if start + len <= from || start >= to {
            continue;
        }
        let (k0, k1) = if forward { (0, 1) } else { (1, 0) };
        let (near, far) = (net.ring_along(c, k0, forward), net.ring_along(c, k1, forward));
        let s = if start <= from { net.frame(e, from) } else { near };
        let t = if start + len > to { near.mix(&far, (to - start) / len) } else { far };
        out.push((s, t, chord.height, chord.section, c, k1));
    }
    out
}

/// Whether end e's head is a rim that hugs its opening, not a head wall with wings.
fn rim(net: &Net, e: usize) -> bool {
    matches!(net.ends[e].part, Part::Rock | Part::Free)
}

/// The metres to the left of end e's axis of the outer edges of its frame: of its opening, or of both openings of a
/// pair.
fn edges(net: &Net, e: usize) -> (f64, f64) {
    let shell = net.rules.ladder.bake.shell_m;
    let edge = half(net, e) + shell;
    match (net.mate[e], net.ends[e].mate) {
        (Some(m), Some(a)) => {
            let other = half(net, m) + shell;
            ((-edge).min(a - other), edge.max(a + other))
        }
        _ => (-edge, edge),
    }
}

/// The inside half-width of end e's tube at its opening.
fn half(net: &Net, e: usize) -> f64 {
    net.opening(e).map_or(net.ends[e].half, |(c, _)| net.chords[c].half)
}

/// The outline of end e's rim, round its opening's outline grown by the frame and the shell.
fn rim_outline(net: &Net, e: usize) -> Vec<Corner> {
    let (end, design) = (&net.ends[e], &net.rules.design);
    let grow = design.frame_m + net.rules.ladder.bake.shell_m;
    let sp = spring(net, end.section, end.height);
    outline(end.section, half(net, e) + grow, end.height + grow, sp, design.arch_segments)
}

/// The metres to the left of end e's axis of the two ends of its head: of its rim, or of its head wall with its wings.
pub fn wide(net: &Net, e: usize) -> (f64, f64) {
    if rim(net, e) {
        let o = rim_outline(net, e);
        return (o[0][0], o[o.len() - 1][0]);
    }
    let (end, (lo, hi)) = (&net.ends[e], edges(net, e));
    (lo - end.right.len() as f64, hi + end.left.len() as f64)
}

/// The top of a head wall `t` metres across to the left of its axis, over the opening's floor: `face` over the frame
/// from `lo` to `hi`, then the rows of the wings, a value per metre out. A wing's top is at most a line that falls to
/// its coping's height `coping` at the end of its row: `slope` metres per metre, or more steeply where the row is too
/// short to fall from `face` at the frame's edge. So the top goes on from the head wall with no step, and where the
/// ground goes on past the row, the coping goes into it and no high face ends the wall.
fn top(end: &crate::End, (lo, hi): (f64, f64), (coping, slope): (f64, f64), t: f64) -> f64 {
    let (row, d) = if t > hi {
        (&end.left, t - hi)
    } else if t < lo {
        (&end.right, lo - t)
    } else {
        return end.face;
    };
    if row.is_empty() {
        return end.face;
    }
    let at = |i: usize| if i == 0 { end.face } else { row[(i - 1).min(row.len() - 1)] };
    let j = d.floor() as usize;
    let along = at(j) + (at(j + 1) - at(j)) * (d - j as f64);
    let len = row.len() as f64;
    along.min(coping + slope.max((end.face - coping) / len) * (len - d))
}

/// End e's opening outline grown by `grow`, from the sill up its side to a corner at the floor's height there, where the
/// tube's wall stands on the floor, and on over its top.
fn grown(net: &Net, e: usize, grow: f64) -> Vec<Corner> {
    let Some((c, _)) = net.opening(e) else { return Vec::new() };
    let (chord, design) = (&net.chords[c], &net.rules.design);
    let pieces = tube::floor_pieces(net, chord);
    let sp = spring(net, chord.section, chord.height);
    let o = outline(chord.section, chord.half + grow, chord.height + grow, sp, design.arch_segments);
    let (first, last) = (o[0][0], o[o.len() - 1][0]);
    let sides = |x: f64, top: f64| [[x, -design.lift_m], [x, top]];
    let mut out = sides(first, pieces[0].2).to_vec();
    out.extend_from_slice(&o[1..o.len() - 1]);
    out.extend(sides(last, pieces[pieces.len() - 1].2).into_iter().rev());
    out
}

/// The cut of a pair at end e's frame line: the metres to the left of its axis of the middle between its tube's wall and
/// the mate's, and the side of the mate (1 to the left).
fn pair_cut(net: &Net, e: usize) -> Option<(f64, f64)> {
    let (m, a) = (net.mate[e]?, net.ends[e].mate?);
    Some(((a + a.signum() * (half(net, e) - half(net, m))) / 2.0, a.signum()))
}

/// The faces of the head at end e: its head wall or rim, the sill, the reveal and the frame round the opening, the dark
/// face in it, and the shell of its collar. The head and the shell go down to one level (`boxes::foot`); the sill lies
/// `lift_m` under the floor, under the road of the tube and of the ground. Faces that meet share the corners of their
/// edges. Each opening of a pair draws the head wall of both with its own hole, on its side of the middle between their
/// walls, and closes the reveal where the two holes differ there.
pub fn faces<L: Fn([f64; 3]) -> f64>(b: &mut Builder<L>, net: &Net, e: usize) {
    let end = &net.ends[e];
    let Some((c, k)) = net.opening(e) else { return };
    let chord = &net.chords[c];
    let look = &net.rules.looks[&end.look];
    let (design, bake) = (&net.rules.design, &net.rules.ladder.bake);
    let (sill, foot) = (-design.lift_m, boxes::foot(net, e));
    let half = chord.half;
    let sp = spring(net, chord.section, chord.height);
    let reach = boxes::reach(net, e);
    let shell = collar(net, e, reach.shell);
    let into = end.way_in();
    let (inward_, outward) = (flat(into), flat([-into[0], -into[1]]));
    if net.closed(e) {
        closed(b, net, (e, c, k), foot);
    } else {
        let pieces = tube::floor_pieces(net, chord);
        let (inner, hole) = (grown(net, e, 0.0), grown(net, e, design.frame_m));
        let (front, ring, back) =
            (net.frame(e, -design.recess_m), net.frame(e, 0.0), net.frame(e, design.face_m - design.recess_m));
        let (h, section) = shell.first().map_or((chord.height, chord.section), |p| (p.2, p.3));
        let cut = pair_cut(net, e);
        b.clip(cut.map(|(x, side)| {
            let x = x + side * tube::OVERLAP_M;
            let (p, q, r) = (front.at([x, 0.0]), back.at([x, 0.0]), front.at([x + side, 0.0]));
            Clip { a: [p[0], p[1]], b: [q[0], q[1]], away: [r[0] - p[0], r[1] - p[1]] }
        }));
        let wide = wide(net, e);
        let (rim, edges) = (rim(net, e), edges(net, e));
        let rim_line = rim_outline(net, e);
        // The rows of a pair are over the floor of its record's opening, the end of the input before its mate.
        let record = net.mate[e].filter(|&m| m < e).map_or(0.0, |m| net.ends[m].z - end.z);
        let top = |t: f64| {
            if rim {
                top_at(&rim_line, t).unwrap_or(0.0)
            } else {
                top(end, edges, (bake.coping_m + record, design.wing_slope), t)
            }
        };
        let (edges, coping, cope) = if rim { (wide, 0.0, look.face) } else { (edges, bake.coping_m, look.coping) };
        let wall = Wall { top: &top, wide, edges, foot, coping, face: look.face, cope };
        // The sill is split where the pieces of the floor meet the front of the tube.
        let hw = hole[0][0].abs();
        let sills: Vec<f64> =
            [-hw].into_iter().chain(pieces.iter().map(|p| p.0)).chain([pieces[pieces.len() - 1].1, hw]).collect();
        let front_top = wall.face(b, &front, &hole, &sills, outward);
        // The back's hole is the outside of the collar's shell, which goes on from it.
        let back_top = wall.face(b, &back, &tube::shell_outline(net, section, half, h, foot - back.z), &[], inward_);
        b.strip((&front, &front_top), (&back, &back_top), cope, [0.0, 0.0, 1.0]);
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
        let rim_v: Vec<u32> = inner.iter().map(|&c| b.at(&st, c, dark)).collect();
        for k in 0..rim_v.len() {
            b.triangle(centre, rim_v[k], rim_v[(k + 1) % rim_v.len()], outward);
        }
        b.clip(None);
        // Where the two holes of a pair differ at the middle, the reveal closes the step, from the front to the frame
        // line: under the higher sill and over the lower top.
        if let (Some((x, side)), Some(m)) = (cut, net.mate[e]) {
            let other = grown(net, m, design.frame_m);
            let span = |st: &Station, s: f64| {
                let df = net.frame(m, s).z - st.z;
                let own = (sill, top_at(&hole, x)?);
                let them = top_at(&other, x - net.ends[e].mate?).map(|t| (sill + df, t + df));
                Some(steps(own, them))
            };
            if let (Some(a), Some(z)) = (span(&front, -design.recess_m), span(&ring, 0.0)) {
                for i in 0..2 {
                    cut_face(
                        b,
                        (&front, &ring),
                        (x, a[i].0, a[i].1),
                        (x, z[i].0, z[i].1),
                        look.face,
                        front.out([-side, 0.0]),
                    );
                }
            }
        }
    }
    let run: Vec<_> = shell.iter().map(|p| (p.0, p.1, p.2, p.3, p.4)).collect();
    tube::shell(b, net, &run, half, foot, look.shell, reach.then);
    if let Some(&(s, t, _, _, c, k)) = shell.last().filter(|_| reach.closed) {
        b.clip(net.clip(c, (&s, &t)).map(|c| c.0));
        end_face(b, net, (c, k), &t, foot, look.shell, flat([t.x - s.x, t.y - s.y]));
        b.clip(None);
    }
}

/// The front of a closed end e at chord c's vertex k: the face that closes its shell on the frame line, and over the
/// inside of its tube a dark face that the viewer always draws, seen from inside.
fn closed<L: Fn([f64; 3]) -> f64>(b: &mut Builder<L>, net: &Net, (e, c, k): (usize, usize, usize), foot: f64) {
    let (end, chord) = (&net.ends[e], &net.chords[c]);
    let (ring, into) = (net.frame(e, 0.0), net.ends[e].way_in());
    let look = &net.rules.looks[&end.look];
    end_face(b, net, (c, k), &ring, foot, look.shell, flat([-into[0], -into[1]]));
    // The room's outline round, from the right wall's foot: up and over, down the left wall, back across the floor.
    let (right, left) = room(net, (c, k));
    let mut round = right;
    round.extend(left[1..left.len() - 1].iter().rev());
    let dark = net.rules.inside.void;
    let centre = b.at(&ring, [0.0, chord.height / 2.0], dark);
    let rim: Vec<u32> = round.iter().map(|&c| b.at(&ring, c, dark)).collect();
    for k in 0..rim.len() {
        b.triangle(centre, rim[k], rim[(k + 1) % rim.len()], flat(into));
    }
}

/// The face that closes the shell at station `t` at vertex k of chord c: from the outside of the shell, down to `foot`,
/// to the inside of the chord there and under its floor; facing `out`. Two strips from the shell's right foot and the
/// inside's right floor corner: one up the right and over the top to the left spring, one across the foot and the
/// floor and up the left to the left spring.
fn end_face<L: Fn([f64; 3]) -> f64>(
    b: &mut Builder<L>,
    net: &Net,
    (c, k): (usize, usize),
    t: &Station,
    foot: f64,
    colour: u32,
    out: [f64; 3],
) {
    let chord = &net.chords[c];
    let shell = tube::shell_outline(net, chord.section, chord.half, chord.height, foot - t.z);
    let n = shell.len();
    let (right, left) = room(net, (c, k));
    b.strip((t, &shell[..n - 1]), (t, &right), colour, out);
    let bottom = [shell[0], shell[n - 1], shell[n - 2]];
    b.strip((t, &bottom), (t, &left), colour, out);
}

/// The room of chord c at its vertex k, as the inside draws it there, in two runs from the right wall's foot to the
/// left spring: up the right wall and over the vault; across the floor (each piece's top, and both tops where two
/// pieces meet) and up the left wall. A wall has the spring of a lower chord that goes on there.
fn room(net: &Net, (c, k): (usize, usize)) -> (Vec<Corner>, Vec<Corner>) {
    let chord = &net.chords[c];
    let sp = spring(net, chord.section, chord.height);
    let inner = outline(chord.section, chord.half, chord.height, sp, net.rules.design.arch_segments);
    let (n, pieces) = (inner.len(), tube::floor_pieces(net, chord));
    let lower = net.lower(c, k).map(|l| spring(net, chord.section, l));
    let wall = |x: f64, floor: f64| {
        let mut w = vec![[x, floor]];
        w.extend(lower.filter(|&l| l > floor && l < sp).map(|l| [x, l]));
        w.push([x, sp]);
        w
    };
    let mut floor: Vec<Corner> = vec![[pieces[0].0, pieces[0].2]];
    for w in pieces.windows(2) {
        floor.push([w[0].1, w[0].2]);
        floor.push([w[1].0, w[1].2]);
    }
    floor.push([pieces[pieces.len() - 1].1, pieces[pieces.len() - 1].2]);
    floor.dedup();
    let right: Vec<Corner> =
        wall(inner[0][0], pieces[0].2).into_iter().chain(inner[2..n - 1].iter().copied()).collect();
    floor.extend(wall(inner[n - 1][0], pieces[pieces.len() - 1].2).into_iter().skip(1));
    (right, floor)
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

/// A face of a head wall: its top over the opening's floor, flat or round between `edges` across and per metre out past
/// them, the metres across of its two ends, the height of its foot, the depth of its coping and its colours.
struct Wall<'a> {
    top: &'a dyn Fn(f64) -> f64,
    wide: (f64, f64),
    edges: (f64, f64),
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
        let (n, hw, foot, (wide, edges)) = (hole.len(), hole[0][0].abs(), self.foot - st.z, (self.wide, self.edges));
        // The side of the hole up to where its top starts.
        let rise = hole.iter().position(|p| p[0] != hole[0][0]).unwrap() - 1;
        let jamb: Vec<f64> = [foot].into_iter().chain(hole[..=rise].iter().map(|p| p[1])).collect();
        let mut columns: Vec<(f64, Side)> =
            (metres(wide.0, edges.0).into_iter().filter(|&t| t < -hw)).map(|t| (t, self.side(st, t, &[]))).collect();
        columns.push((-hw, self.side(st, -hw, &jamb)));
        let over: Vec<(f64, Side)> =
            hole[rise..n - rise].iter().map(|p| (p[0], self.side(st, p[0], &[p[1]]))).collect();
        let mut left = vec![(hw, self.side(st, hw, &jamb))];
        left.extend((metres(edges.1, wide.1).into_iter().filter(|&t| t > hw)).map(|t| (t, self.side(st, t, &[]))));
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
