//! The head of a record on its frame line, with a hole per opening. An arch: a head wall with a frame round each hole
//! and wings whose tops follow the rows of the bake. A rim: of openings in the rock and of free tube ends, the head
//! hugs each hole. Then per hole the outer shell of its part and the dark face in it. A free end covered at its
//! opening ends in the hill, closed by a dark face, with no head.

use crate::boxes;
use crate::mesh::{flagged, flat, outline, Builder, Corner, Station, VOID};
use crate::seam;
use crate::tube::{self, direction, inward, spring, top_at, Net};
use crate::{Part, Section};

/// A piece of the shell: its start and end station across to the left of the way in, its chord's height at each and
/// its section, and the chord and its vertex at the end.
pub type Piece = (Station, Station, [f64; 2], Section, usize, usize);

/// The pieces of the shell of hole e: per chord from the back of the head wall, or from the frame line of a closed
/// end, to `to` metres in.
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
        let height = |u: f64| chord.height[k0] + (chord.height[k1] - chord.height[k0]) * u.clamp(0.0, 1.0);
        let h = [height((from - start) / len), height((to - start) / len)];
        out.push((s, t, h, chord.section, c, k1));
    }
    out
}

/// Whether record r's head is rims that hug its holes: each hole opens in the rock or is a free end.
fn rim(net: &Net, r: usize) -> bool {
    net.of_record[r].iter().all(|&e| matches!(net.holes[e].part, Part::Rock | Part::Free))
}

/// The inside half-width of hole e's tube at its opening.
fn half_at(net: &Net, e: usize) -> f64 {
    net.opening(e).map_or(net.holes[e].half, |(c, _)| net.chords[c].half)
}

/// The arch segments of hole e's tube at its opening.
fn segments_at(net: &Net, e: usize) -> usize {
    net.opening(e).map_or(net.rules.design.arch_segments, |(c, _)| tube::segments(net, &net.chords[c]))
}

/// The outline of hole e's rim, round its opening's outline grown by the frame and the shell.
fn rim_outline(net: &Net, e: usize) -> Vec<Corner> {
    let (o, design) = (&net.holes[e], &net.rules.design);
    let grow = design.frame_m + net.rules.ladder.bake.shell_m;
    let sp = spring(net, o.section, o.height);
    outline(o.section, half_at(net, e) + grow, o.height + grow, sp, segments_at(net, e))
}

/// The metres to the left of record r's point of the outer edges of the frames of its holes.
fn edges(net: &Net, r: usize) -> (f64, f64) {
    let shell = net.rules.ladder.bake.shell_m;
    let span =
        |e: usize| (net.holes[e].across - half_at(net, e) - shell, net.holes[e].across + half_at(net, e) + shell);
    (net.of_record[r].iter().map(|&e| span(e)))
        .fold((f64::INFINITY, f64::NEG_INFINITY), |a, s| (a.0.min(s.0), a.1.max(s.1)))
}

/// The parts of record r's head across, metres to the left of its point, each with its holes from the right: of a
/// head wall, the wall with its wings; of rims, each run of rims that overlap.
fn spans(net: &Net, r: usize) -> Vec<((f64, f64), Vec<usize>)> {
    let (end, list) = (&net.records[r], &net.of_record[r]);
    if !rim(net, r) {
        let (lo, hi) = edges(net, r);
        return vec![((lo - end.right.len() as f64, hi + end.left.len() as f64), list.clone())];
    }
    let mut out: Vec<((f64, f64), Vec<usize>)> = Vec::new();
    for &e in list {
        let (o, across) = (rim_outline(net, e), net.holes[e].across);
        let (lo, hi) = (across + o[0][0], across + o[o.len() - 1][0]);
        match out.last_mut() {
            Some(((_, end), holes)) if lo <= *end => {
                *end = end.max(hi);
                holes.push(e);
            }
            _ => out.push(((lo, hi), vec![e])),
        }
    }
    out
}

/// The metres to the left of hole e's axis of the two ends of the part of its head that holds it.
pub fn wide(net: &Net, e: usize) -> (f64, f64) {
    let o = &net.holes[e];
    let span = spans(net, o.record).into_iter().find(|(_, list)| list.contains(&e)).map_or((0.0, 0.0), |s| s.0);
    (span.0 - o.across, span.1 - o.across)
}

/// The top of a head wall `t` metres across to the left of its record's point, over the record's floor: `face` over
/// the frames from `lo` to `hi`, then the rows of the wings, a value per metre out. A wing's top is at most a line that
/// falls to its coping's height `coping` at the end of its row: `slope` metres per metre, or more steeply where the
/// row is too short to fall from `face` at the frame's edge. So the top goes on from the head wall with no step, and
/// where the ground goes on past the row, the coping goes into it and no high face ends the wall.
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

/// Hole e's opening outline grown by `grow`, from the sill up its side to a corner at the floor's height there, where
/// the tube's wall stands on the floor, and on over its top.
fn grown(net: &Net, e: usize, grow: f64) -> Vec<Corner> {
    let Some((c, k)) = net.opening(e) else { return Vec::new() };
    let (chord, design) = (&net.chords[c], &net.rules.design);
    let pieces = floor_across(net, e);
    let sp = spring(net, chord.section, chord.height[k]);
    let o = outline(chord.section, chord.half + grow, chord.height[k] + grow, sp, tube::segments(net, chord));
    let (first, last) = (o[0][0], o[o.len() - 1][0]);
    let sides = |x: f64, top: f64| [[x, -design.lift_m], [x, top]];
    let mut out = sides(first, pieces[0].2).to_vec();
    out.extend_from_slice(&o[1..o.len() - 1]);
    out.extend(sides(last, pieces[pieces.len() - 1].2).into_iter().rev());
    out
}

/// The floor of hole e's tube at its opening across to the left of the way in: per piece from the right, its metres
/// across and the height of its top.
fn floor_across(net: &Net, e: usize) -> Vec<(f64, f64, f64)> {
    net.opening(e).map_or(Vec::new(), |(c, k)| tube::floor_along(net, c, k, &net.frame(e, 0.0)))
}

/// The faces of record r: the head on its frame line with a hole per opening (its head wall or rims, the sills, the
/// reveals and the frames round the holes, the dark faces in them), and per hole the shell of its collar. The head and
/// the shells go down to one level (`boxes::foot`); a sill lies `lift_m` under its floor, under the road of the tube
/// and of the ground. Faces that meet share the corners of their edges.
pub fn faces<L: Fn([f64; 3]) -> f64>(b: &mut Builder<L>, net: &Net, r: usize) {
    let end = &net.records[r];
    let list = &net.of_record[r];
    let look = &net.rules.looks[&end.look];
    let foot = boxes::foot(net, list[0]);
    let reaches: Vec<boxes::Reach> = list.iter().map(|&e| boxes::reach(net, e)).collect();
    for &e in list.iter().filter(|&&e| net.closed(e)) {
        closed(b, net, e, foot);
    }
    if list.iter().any(|&e| !net.closed(e)) {
        head(b, net, r, (foot, &reaches));
    }
    for (&e, &reach) in list.iter().zip(&reaches) {
        let shell = collar(net, e, reach.shell);
        let half = net.opening(e).map_or(net.holes[e].half, |(c, _)| net.chords[c].half);
        let run: Vec<_> = shell.iter().map(|p| (p.0, p.1, p.2, p.3)).collect();
        tube::shell(b, net, &run, (half, segments_at(net, e)), foot, look.shell, reach.then);
        if let Some(&(s, t, _, _, c, k)) = shell.last().filter(|_| reach.closed) {
            end_face(b, net, (c, k), &t, foot, look.shell, flat([t.x - s.x, t.y - s.y]));
        }
    }
}

/// The head of record r, its open holes in its wall or rims, from the foot `foot` up.
fn head<L: Fn([f64; 3]) -> f64>(b: &mut Builder<L>, net: &Net, r: usize, (foot, reaches): (f64, &[boxes::Reach])) {
    let end = &net.records[r];
    let look = &net.rules.looks[&end.look];
    let (design, bake) = (&net.rules.design, &net.rules.ladder.bake);
    let sill = -design.lift_m;
    let into = end.way_in();
    let (inward_, outward) = (flat(into), flat([-into[0], -into[1]]));
    let (front, back) = (net.line(r, -design.recess_m), net.line(r, design.face_m - design.recess_m));
    let rim = rim(net, r);
    let open = |e: &usize| !net.closed(*e);
    for (wide, list) in spans(net, r) {
        let list: Vec<usize> = list.into_iter().filter(open).collect();
        let rims: Vec<(f64, f64, Vec<Corner>)> =
            list.iter().map(|&e| (net.holes[e].across, net.holes[e].up, rim_outline(net, e))).collect();
        let edges = edges(net, r);
        let top = |t: f64| {
            if rim {
                let tops = rims.iter().filter_map(|(across, up, o)| top_at(o, t - across).map(|v| v + up));
                tops.fold(None, |a: Option<f64>, v| Some(a.map_or(v, |a| a.max(v)))).unwrap_or(0.0)
            } else {
                top(end, edges, (bake.coping_m, design.wing_slope), t)
            }
        };
        let (edges, coping, cope) = if rim { (wide, 0.0, look.face) } else { (edges, bake.coping_m, look.coping) };
        let wall = Wall { top: &top, wide, edges, foot, coping, face: look.face, cope };
        // Per hole its frame's outline at the front, under it the sill split where the pieces of the floor meet the
        // front of the tube; at the back the outside of the collar's shell, which goes on from it.
        let at_front: Vec<Gap> = (list.iter())
            .map(|&e| {
                let hole = grown(net, e, design.frame_m);
                let hw = hole[0][0].abs();
                let pieces = floor_across(net, e);
                let sills = [-hw].into_iter().chain(pieces.iter().map(|p| p.0)).chain([pieces[pieces.len() - 1].1, hw]);
                Gap {
                    st: net.frame(e, -design.recess_m),
                    across: net.holes[e].across,
                    up: net.holes[e].up,
                    outline: hole,
                    sills: sills.collect(),
                }
            })
            .collect();
        let at_back: Vec<Gap> = (list.iter())
            .map(|&e| {
                let st = net.frame(e, design.face_m - design.recess_m);
                let (c, k) = net.opening(e).expect("a built hole");
                let chord = &net.chords[c];
                let reach = reaches[net.of_record[r].iter().position(|&h| h == e).expect("a hole of the record")];
                let (h, section) =
                    collar(net, e, reach.shell).first().map_or((chord.height[k], chord.section), |p| (p.2[0], p.3));
                let outline = tube::shell_outline(net, (section, segments_at(net, e)), chord.half, h, foot - st.z);
                Gap { st, across: net.holes[e].across, up: net.holes[e].up, outline, sills: Vec::new() }
            })
            .collect();
        let front_top = wall.face(b, &front, &at_front, outward);
        let back_top = wall.face(b, &back, &at_back, inward_);
        b.strip_at((&front_top.0, &front_top.1), (&back_top.0, &back_top.1), cope, [0.0, 0.0, 1.0]);
        for (t, side) in [(wide.0, -1.0), (wide.1, 1.0)] {
            wall.column(
                b,
                (&front, t, &wall.side(&front, t, 0.0, &[])),
                (&back, t, &wall.side(&back, t, 0.0, &[])),
                front.out([side, 0.0]),
            );
        }
    }
    for e in net.of_record[r].iter().copied().filter(|e| open(e)) {
        let (c, k) = net.opening(e).expect("a built hole");
        let (chord, height) = (&net.chords[c], net.chords[c].height[k]);
        let sp = spring(net, chord.section, height);
        let pieces = floor_across(net, e);
        let (inner, hole) = (grown(net, e, 0.0), grown(net, e, design.frame_m));
        let (front, ring) = (net.frame(e, -design.recess_m), net.frame(e, 0.0));
        // The sill from the front to the frame line, the reveal, and the frame round the opening.
        let hw = hole[0][0].abs();
        let sills: Vec<f64> =
            [-hw].into_iter().chain(pieces.iter().map(|p| p.0)).chain([pieces[pieces.len() - 1].1, hw]).collect();
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
        let centre = b.at(&st, [0.0, height / 2.0], dark);
        let rim_v: Vec<u32> = inner.iter().map(|&c| b.at(&st, c, dark)).collect();
        for k in 0..rim_v.len() {
            b.triangle(centre, rim_v[k], rim_v[(k + 1) % rim_v.len()], outward);
        }
    }
}

/// The front of a closed hole e: the face that closes its shell on the frame line, and over the inside of its tube a
/// dark face that the viewer always draws, seen from inside.
fn closed<L: Fn([f64; 3]) -> f64>(b: &mut Builder<L>, net: &Net, e: usize, foot: f64) {
    let Some((c, k)) = net.opening(e) else { return };
    let (end, chord) = (&net.holes[e], &net.chords[c]);
    let (ring, into) = (net.frame(e, 0.0), end.way_in());
    let look = &net.rules.looks[&net.records[end.record].look];
    end_face(b, net, (c, k), &ring, foot, look.shell, flat([-into[0], -into[1]]));
    // The room's outline round, from the right wall's foot: up and over, down the left wall, back across the floor.
    let (right, left) = room(net, (c, k), &ring);
    let mut round = right;
    round.extend(left[1..left.len() - 1].iter().rev());
    let dark = net.rules.inside.void;
    let centre = b.at(&ring, [0.0, chord.height[k] / 2.0], dark);
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
    let (section, segments) = (chord.section, tube::segments(net, chord));
    let shell = tube::shell_outline(net, (section, segments), chord.half, chord.height[k], foot - t.z);
    let n = shell.len();
    let (right, left) = room(net, (c, k), t);
    let bottom = [shell[0], shell[n - 1], shell[n - 2]];
    // At a seam the floor has a corner at each hole's side and step: the two halves are cut into ears, as the seam face
    // is, so no corner lies on an edge of a strip.
    if net.seam_at(c, k).is_some() {
        for (outer, inner) in [(&shell[..n - 1], &right), (&bottom[..], &left)] {
            let poly: Vec<Corner> = outer.iter().chain(inner.iter().rev()).copied().collect();
            for tri in seam::ear_clip(&poly) {
                let v = tri.map(|i| b.at(t, poly[i], colour));
                b.triangle(v[0], v[1], v[2], out);
            }
        }
        return;
    }
    b.strip((t, &shell[..n - 1]), (t, &right), colour, out);
    b.strip((t, &bottom), (t, &left), colour, out);
}

/// The room of chord c at its vertex k, as the inside draws it there, across to the left of station `st` there, in two
/// runs from the right wall's foot to the left spring: up the right wall and over the vault; across the floor (each
/// piece's top, and both tops where two pieces meet) and up the left wall. A wall has the spring of a lower chord that
/// goes on there.
fn room(net: &Net, (c, k): (usize, usize), st: &Station) -> (Vec<Corner>, Vec<Corner>) {
    let chord = &net.chords[c];
    // At a seam before which the room ends, its floor and its outline with the seam's corners.
    if let Some((r, 0)) = net.seam_at(c, k) {
        if let Some(cut) = net.seam_cut(r) {
            let line = net.line(r, 0.0);
            let sign = if st.ax * line.ax + st.ay * line.ay >= 0.0 { 1.0 } else { -1.0 };
            let mine = |list: &[(Corner, [f64; 3])]| {
                let mut out: Vec<Corner> = list.iter().map(|q| [sign * q.0[0], q.0[1]]).collect();
                if sign < 0.0 {
                    out.reverse();
                }
                out
            };
            let (floor, top) = (mine(&cut.floor), mine(&cut.top));
            // On each wall the corners of a hole's side there, as the inside's wall has them.
            let wall = |from: Corner, to: Corner| {
                let mut on: Vec<Corner> = (cut.holes.iter().flatten())
                    .map(|q| [sign * q.0[0], q.0[1]])
                    .filter(|q| q[0] == from[0] && q[1] > from[1] && q[1] < to[1])
                    .collect();
                on.sort_by(|p, q| p[1].total_cmp(&q[1]));
                on
            };
            let (first, last, spring_r, spring_l) = (floor[0], floor[floor.len() - 1], top[0], top[top.len() - 1]);
            let right = [first].into_iter().chain(wall(first, spring_r)).chain(top.iter().copied()).collect();
            let left = floor.iter().copied().chain(wall(last, spring_l)).chain([spring_l]).collect();
            return (right, left);
        }
    }
    let sp = spring(net, chord.section, chord.height[k]);
    let inner = outline(chord.section, chord.half, chord.height[k], sp, tube::segments(net, chord));
    let (n, pieces) = (inner.len(), tube::floor_along(net, c, k, st));
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

/// A hole in a face of a head: its station on that face, its place in the record (`across`, `up`), its outline there
/// from the bottom of its right side up and over its top to the bottom of its left side, and where its sill splits.
struct Gap {
    st: Station,
    across: f64,
    up: f64,
    outline: Vec<Corner>,
    sills: Vec<f64>,
}

/// A face of a head wall: its top over the record's floor, by the metres to the left of its point, flat or round
/// between `edges` across and per metre out past them, the metres across of its two ends, the height of its foot, the
/// depth of its coping and its colours.
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

/// A column of a wall: its station, its metres across there and across the record, the height of that station's floor
/// over the record's, and its side over that floor.
struct Column {
    st: Station,
    x: f64,
    across: f64,
    up: f64,
    side: Side,
}

impl Wall<'_> {
    /// The side of a column at `x` across the record at station `st`, whose floor lies `up` over the record's: the
    /// corners from the foot, or from `bottoms` where the column stands on a hole (their heights up), to the coping's
    /// foot and the top, both at least the highest bottom.
    fn side(&self, st: &Station, x: f64, up: f64, bottoms: &[f64]) -> Side {
        let mut ups: Vec<f64> = if bottoms.is_empty() { vec![self.foot - st.z] } else { bottoms.to_vec() };
        let low = ups[ups.len() - 1];
        let top = ((self.top)(x) - up).max(low);
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

    /// The face at the record's station `st` from end to end, round its holes from the right: columns from the foot up
    /// beside each hole, columns from a hole up over it, each on the hole's own station, and under each hole the sill,
    /// split at its `sills`. Gives the corners of the top as world points, with their metres across the record and up
    /// from its floor.
    fn face<L: Fn([f64; 3]) -> f64>(
        &self,
        b: &mut Builder<L>,
        st: &Station,
        holes: &[Gap],
        out: [f64; 3],
    ) -> (Vec<[f64; 3]>, Vec<Corner>) {
        let (wide, edges) = (self.wide, self.edges);
        let plain = |x: f64| Column { st: *st, x, across: x, up: 0.0, side: self.side(st, x, 0.0, &[]) };
        let on = |h: &Gap, x: f64, bottoms: &[f64]| {
            let across = h.across + x;
            Column { st: h.st, x, across, up: h.up, side: self.side(&h.st, across, h.up, bottoms) }
        };
        // Runs of columns that join, from the right: beside and between the holes, and over each hole.
        let mut runs: Vec<(Vec<Column>, bool)> = Vec::new();
        let first = holes.first().map_or(wide.1, |h| h.across + h.outline[0][0]);
        let mut beside: Vec<Column> = metres(wide.0, edges.0).into_iter().filter(|&t| t < first).map(plain).collect();
        for h in holes {
            let (n, hw, foot) = (h.outline.len(), h.outline[0][0].abs(), self.foot - h.st.z);
            // Each side of the hole up to where its top starts.
            let rise = h.outline.iter().position(|p| p[0] != h.outline[0][0]).unwrap() - 1;
            let jamb = |side: &mut dyn Iterator<Item = &Corner>| {
                [foot].into_iter().chain(side.map(|p| p[1])).collect::<Vec<f64>>()
            };
            beside.push(on(h, -hw, &jamb(&mut h.outline[..=rise].iter())));
            runs.push((beside, false));
            runs.push((h.outline[rise..n - rise].iter().map(|p| on(h, p[0], &[p[1]])).collect(), true));
            beside = vec![on(h, hw, &jamb(&mut h.outline[n - 1 - rise..].iter().rev()))];
        }
        let last = holes.last().map_or(wide.0, |h| h.across - h.outline[0][0]);
        beside.extend(metres(edges.1, wide.1).into_iter().filter(|&t| t > last).map(plain));
        runs.push((beside, false));
        for (list, _) in &runs {
            for w in list.windows(2) {
                self.column(b, (&w[0].st, w[0].x, &w[0].side), (&w[1].st, w[1].x, &w[1].side), out);
            }
        }
        for h in holes {
            let foot = self.foot - h.st.z;
            if h.outline[0][1] > foot {
                for w in h.sills.windows(2) {
                    let (low, high) =
                        ([[w[0], foot], [w[1], foot]], [[w[0], h.outline[0][1]], [w[1], h.outline[0][1]]]);
                    b.strip((&h.st, &low), (&h.st, &high), self.face, out);
                }
            }
        }
        // The tops: a column over a hole at its side is the column beside it.
        let tops = runs.iter().flat_map(|(list, over)| if *over { &list[1..list.len() - 1] } else { &list[..] });
        tops.map(|c| {
            let top = c.side.0[c.side.0.len() - 1];
            (c.st.at([c.x, top]), [c.across, top + c.up])
        })
        .unzip()
    }
}
