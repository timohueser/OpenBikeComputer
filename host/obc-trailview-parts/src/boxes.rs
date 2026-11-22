//! The boxes of a part, in which the ground does not draw. Together they hold the room of the opening and of the tube,
//! and each lies inside the part's wall, at its middle; and how far a part runs into its tube.

use crate::mesh::{left_of, Station};
use crate::tube::{direction, spring, Meets, Net};
use crate::{metres_per_unit, Part, Section};

/// Words of a box: the middle of its start (world units), its unit way in, its depth and its half-width (m), the
/// heights of its roof, its floor and its spring on its start plane, their rise per metre in, and the slopes of its
/// start and of its end: a point `c` metres to the left of its axis lies in the box from `start · c` metres in to
/// `depth + end · c`. Its heights rise between its two end planes as a tube's do between its rings: a point `along`
/// metres in lies the share `(along − start · c) / (depth + (end − start) · c)` of the way, and rises by that share of
/// `rise · depth`; so each end is level across. Over its spring the roof is round, a half-ellipse from the half-width
/// to the roof; where the spring is the roof the box is flat.
pub const BOX: usize = 12;

/// How far hole e's part runs in from its frame line: its boxes, and its shell, which ends with a face (`closed`) or
/// meets the shell of the tube's other end (`then`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reach {
    pub boxes: f64,
    pub shell: f64,
    pub closed: bool,
    pub then: Meets,
}

/// The metres from end e's frame line that its part must reach: as today's box of a portal, a level-17 cell's
/// diagonal and a metre past the start of the cover over the tube, `portal_cells` cells in or a metre past the
/// collar. An underpass is one part for its chain: a `box` end runs to where it meets the other end's part, as the
/// ground can dip between two ways on top.
fn need(net: &Net, e: usize) -> f64 {
    let (end, design) = (&net.holes[e], &net.rules.design);
    if end.part == Part::Box {
        return f64::INFINITY;
    }
    let cell = design.mesh_texels * 32.0 * metres_per_unit(end.at[1]);
    (design.portal_cells * cell).max(end.collar + 1.0) + std::f64::consts::SQRT_2 * cell + 1.0
}

/// The metres in from end e's opening of each vertex of its walk to the other end of its tube: 0, then the far vertex
/// of each chord. Behind a seam a walk ends at the vertex before it (the bake writes one): the seam face lies in the
/// seam's plane, and only the room before the line closes a shell there.
fn stops(net: &Net, e: usize) -> Vec<f64> {
    let walk = net.walk(e, f64::INFINITY);
    let mut out = vec![0.0];
    out.extend(walk.iter().map(|&(c, _, start)| start + direction(&net.chords[c]).1));
    let last = walk.last().and_then(|&(c, forward, _)| net.seam_at(c, usize::from(forward)));
    if last.is_some_and(|(_, h)| h > 0) {
        out.pop();
    }
    out
}

/// The first stop at `metres` or past it; the last stop where none is.
fn at_or_past(stops: &[f64], metres: f64) -> f64 {
    stops.iter().copied().find(|&s| s >= metres).unwrap_or(stops[stops.len() - 1])
}

/// The reach of built end e. Its part runs to the first vertex of its tube at `need` or past it. Where the parts of the
/// tube's two ends would meet, each ends at the vertex nearest the middle, or in a tube of one chord at the middle of
/// that chord, its boxes and its shell, and there the taller shell closes the step to the lower one; else a face closes
/// each shell.
pub fn reach(net: &Net, e: usize) -> Reach {
    let walk = net.walk(e, f64::INFINITY);
    let at = stops(net, e);
    let (total, own) = (at[at.len() - 1], at_or_past(&at, need(net, e)));
    // A seam has no part: a part that runs to one ends there with a face, on the seam's corners.
    let other = walk.last().and_then(|&(c, forward, _)| net.end_at(c, usize::from(forward)));
    let other = other.filter(|&o| net.holes[o].part != Part::Seam);
    let alone = Reach { boxes: own, shell: own, closed: true, then: Meets::default() };
    let Some(o) = other.filter(|&o| o != e) else { return alone };
    let theirs = at_or_past(&stops(net, o), need(net, o));
    if own + theirs < total {
        return alone;
    }
    // The vertex nearest the middle; of two as near, the one with the least key, so both ends find the same.
    let vertex = |k: usize| {
        let (c, forward, _) = walk[k - 1];
        let ch = &net.chords[c];
        let v = if forward { ch.b } else { ch.a };
        [v[0].to_bits(), v[1].to_bits()]
    };
    let mid = (1..at.len() - 1).min_by(|&i, &j| {
        let (di, dj) = ((at[i] - total / 2.0).abs(), (at[j] - total / 2.0).abs());
        if (di - dj).abs() < 1e-6 {
            vertex(i).cmp(&vertex(j))
        } else {
            di.total_cmp(&dj)
        }
    });
    let (meet, lower) = match mid {
        Some(mid) => {
            let (here, there) = (&net.chords[walk[mid - 1].0], &net.chords[walk[mid].0]);
            (
                at[mid],
                (there.height < here.height && there.section == here.section).then_some((there.height, there.section)),
            )
        }
        None => (total / 2.0, None),
    };
    let foot = Some(foot(net, o));
    Reach { boxes: meet, shell: meet, closed: false, then: Meets { foot, lower } }
}

/// The level that the head and the shells of hole e's record go down to, a world height: its `foot`, or `foot_m` under
/// the lowest floor of the chords that the parts of its holes run over, the lower.
pub fn foot(net: &Net, e: usize) -> f64 {
    let r = net.holes[e].record;
    let end = &net.records[r];
    let low = (net.of_record[r].iter())
        .flat_map(|&e| net.walk(e, at_or_past(&stops(net, e), need(net, e))))
        .map(|(c, ..)| net.chords[c].a[2].min(net.chords[c].b[2]))
        .fold(f64::INFINITY, f64::min);
    (end.z + end.foot).min(low - net.rules.ladder.bake.foot_m)
}

/// The slope of a plane through a point, in the vertical plane of station `ring` (across it), as the slope of a box
/// end along the unit direction `u`: `−(left · n) / (u · n)` for the plane's horizontal normal n.
pub(crate) fn slope(u: [f64; 2], ring: &Station) -> f64 {
    let n = [ring.ay, -ring.ax];
    let lu = left_of(u);
    -(lu[0] * n[0] + lu[1] * n[1]) / (u[0] * n[0] + u[1] * n[1])
}

/// A vertex of the axis that the boxes follow: its world point and floor, and the ring there.
#[derive(Clone, Copy)]
struct Axis {
    p: [f64; 2],
    z: f64,
    ring: Station,
}

/// The vertices of each straight piece of an axis: the first and the last, the points where `cut` says that the section
/// changes, and between them the points that split a piece while it is off by more than `tol` metres (Douglas-Peucker
/// in metres). A point between is off by its distance across from the piece's line, and by the height of its floor
/// over the box's rising line (`Line::off`), on the axis and at both sides of the room.
fn pieces(axis: &[Axis], cut: &[bool], tol: f64, m: f64, half: f64) -> Vec<usize> {
    fn split(axis: &[Axis], a: usize, b: usize, (tol, m, half): (f64, f64, f64), out: &mut Vec<usize>) {
        let line = Line::of(&axis[a], &axis[b], m);
        let error = |k: usize| {
            line.across(&axis[k], m).abs().max(line.off(&axis[k], m, half).iter().fold(0.0f64, |e, d| e.max(d.abs())))
        };
        if let Some(k) = (a + 1..b).max_by(|&i, &j| error(i).total_cmp(&error(j))).filter(|&k| error(k) > tol) {
            split(axis, a, k, (tol, m, half), out);
            out.push(k);
            split(axis, k, b, (tol, m, half), out);
        }
    }
    let mut out = vec![0];
    let mut a = 0;
    for b in (1..axis.len()).filter(|&b| b + 1 == axis.len() || cut[b]) {
        split(axis, a, b, (tol, m, half), &mut out);
        out.push(b);
        a = b;
    }
    out
}

/// The line of a piece of the axis from vertex a to vertex b: its start, its unit way, its depth (m), its rise per
/// metre, and the slopes of its end planes, the planes of the rings at a and at b.
struct Line {
    start: [f64; 2],
    z: f64,
    u: [f64; 2],
    depth: f64,
    rise: f64,
    ends: [f64; 2],
}

impl Line {
    fn of(a: &Axis, b: &Axis, m: f64) -> Line {
        let d = [b.p[0] - a.p[0], b.p[1] - a.p[1]];
        let l = libm::hypot(d[0], d[1]).max(f64::MIN_POSITIVE);
        let (u, depth) = ([d[0] / l, d[1] / l], l * m);
        let rise = (b.z - a.z) / depth.max(f64::MIN_POSITIVE);
        Line { start: a.p, z: a.z, u, depth, rise, ends: [slope(u, &a.ring), slope(u, &b.ring)] }
    }

    /// The metres of a vertex along the line, and across it to the left.
    fn frame(&self, q: &Axis, m: f64) -> (f64, f64) {
        let d = [(q.p[0] - self.start[0]) * m, (q.p[1] - self.start[1]) * m];
        let l = left_of(self.u);
        (d[0] * self.u[0] + d[1] * self.u[1], d[0] * l[0] + d[1] * l[1])
    }

    fn across(&self, q: &Axis, m: f64) -> f64 {
        self.frame(q, m).1
    }

    /// The height of a vertex's floor over the rising line of the box, on its ring at the right side of the room, on
    /// the axis and at the left side (`half` across).
    fn off(&self, q: &Axis, m: f64, half: f64) -> [f64; 3] {
        let (along, _) = self.frame(q, m);
        let k = slope(self.u, &q.ring);
        [-half, 0.0, half].map(|c| {
            let t = share(along + k * c, c, self.depth, self.ends);
            q.z - (self.z + self.rise * self.depth * t)
        })
    }
}

/// The boxes of built end e: the opening's, from the front of the head wall to the frame line, its outline grown to
/// the middle of the wall round it, but at a closed end; then one per straight piece of the tube's axis out to its
/// reach, each at the middle of the wall of its section, its ends in the planes of the tube's rings there.
pub fn boxes(net: &Net, e: usize) -> Vec<[f64; BOX]> {
    let Some((c0, _)) = net.opening(e) else { return Vec::new() };
    let (end, design, bake) = (&net.holes[e], &net.rules.design, &net.rules.ladder.bake);
    let (wall, lift) = (bake.shell_m / 2.0, design.lift_m);
    let first = &net.chords[c0];
    let (front, ring) = (net.frame(e, -design.recess_m), net.frame(e, 0.0));
    let into = end.way_in();
    let roof_of = |z: f64, h: f64, sp: f64, section: Section, grow: f64| {
        let roof = z + h + grow;
        (roof, if section == Section::Box { roof } else { z + sp })
    };
    let sp = spring(net, first.section, first.height);
    let (roof, spring_at) = roof_of(front.z, first.height, sp, first.section, design.frame_m + wall);
    let (half, rise) = (first.half + design.frame_m + wall, (ring.z - front.z) / design.recess_m);
    let opening =
        [front.x, front.y, into[0], into[1], design.recess_m, half, roof, front.z + lift, rise, spring_at, 0.0, 0.0];
    // A closed end has no opening.
    let mut out = if net.closed(e) { Vec::new() } else { vec![opening] };
    // The axis from the frame line: per chord of the walk its far vertex, with the height and section that end there.
    let reach = reach(net, e).boxes;
    let mut axis = vec![Axis { p: end.at, z: end.z, ring }];
    let mut sections = vec![(first.height, first.section, first.half)];
    for (c, forward, start) in net.walk(e, reach) {
        let chord = &net.chords[c];
        let (v, len) = (if forward { chord.b } else { chord.a }, direction(chord).1);
        let far = net.ring_along(c, usize::from(forward), forward);
        // A reach inside a chord ends at a ring there, as the shell does (`arch::collar`).
        let end = if start + len > reach {
            let ring = net.ring_along(c, usize::from(!forward), forward).mix(&far, (reach - start) / len);
            Axis { p: [ring.x, ring.y], z: ring.z, ring }
        } else {
            Axis { p: [v[0], v[1]], z: v[2], ring: far }
        };
        axis.push(end);
        sections.push((chord.height, chord.section, chord.half));
        if start + len >= reach {
            break;
        }
    }
    // A section changes at a vertex where the chords before and after it differ.
    let cut: Vec<bool> =
        (0..axis.len()).map(|k| k > 0 && k + 1 < axis.len() && sections[k] != sections[k + 1]).collect();
    let m = metres_per_unit(end.at[1]);
    let half = sections.iter().map(|s| s.2).fold(0.0, f64::max);
    let keep = pieces(&axis, &cut, bake.shell_m / 4.0, m, half);
    for w in keep.windows(2) {
        let (a, b) = (&axis[w[0]], &axis[w[1]]);
        let (h, section, room) = sections[w[0] + 1];
        let line = Line::of(a, b, m);
        let (u, depth, rise, [start, end]) = (line.u, line.depth, line.rise, line.ends);
        // The floors of the vertices between, on the axis and at the sides of the room, against the box's rising line:
        // the roof takes their middle, the floor their least.
        let offs = axis[w[0] + 1..w[1]].iter().flat_map(|q| line.off(q, m, room));
        let (low, high) = offs.fold((0.0f64, 0.0f64), |(lo, hi), x| (lo.min(x), hi.max(x)));
        let mid = a.z + (low + high) / 2.0;
        let sp = spring(net, section, h);
        let (roof, spring_at) = roof_of(mid, h, sp, section, wall);
        out.push([a.p[0], a.p[1], u[0], u[1], depth, room + wall, roof, a.z + low + lift, rise, spring_at, start, end]);
    }
    out
}

/// The four corners of a box in plan, world units: at its start and at its end, right and left.
pub fn corners(b: &[f64; BOX]) -> [[f64; 2]; 4] {
    let m = metres_per_unit(b[1]);
    let (u, lu) = ([b[2], b[3]], left_of([b[2], b[3]]));
    let at = |along: f64, c: f64| [b[0] + (u[0] * along + lu[0] * c) / m, b[1] + (u[1] * along + lu[1] * c) / m];
    let half = b[5];
    [at(-b[10] * half, -half), at(b[10] * half, half), at(b[4] - b[11] * half, -half), at(b[4] + b[11] * half, half)]
}

/// How far a world point lies outside a box in plan, in about metres; negative inside.
pub fn outside_plan(b: &[f64; BOX], p: [f64; 2]) -> f64 {
    let (along, left) = box_frame(b, p);
    (b[10] * left - along).max(along - b[4] - b[11] * left).max(left.abs() - b[5])
}

/// The share of the way from a box's start plane to its end plane of a point `along` metres in and `left` metres to
/// the left of its axis; the box is `depth` deep and its ends have the slopes `ends`.
pub(crate) fn share(along: f64, left: f64, depth: f64, ends: [f64; 2]) -> f64 {
    (along - ends[0] * left) / (depth + (ends[1] - ends[0]) * left).max(f64::MIN_POSITIVE)
}

/// How far a box's heights have risen at a world point: by its share between the end planes.
fn risen(b: &[f64; BOX], along: f64, left: f64) -> f64 {
    b[8] * b[4] * share(along, left, b[4], [b[10], b[11]])
}

/// A world point's metres along a box's way in from its start, and to the left of it.
fn box_frame(b: &[f64; BOX], p: [f64; 2]) -> (f64, f64) {
    let m = metres_per_unit(b[1]);
    let d = [(p[0] - b[0]) * m, (p[1] - b[1]) * m];
    let lu = left_of([b[2], b[3]]);
    (d[0] * b[2] + d[1] * b[3], d[0] * lu[0] + d[1] * lu[1])
}

/// How far a world point lies outside the sides and the roof of a box, in about metres; negative inside.
pub fn outside_wall(b: &[f64; BOX], p: [f64; 3]) -> f64 {
    let (along, left) = box_frame(b, [p[0], p[1]]);
    let z = p[2] - risen(b, along, left);
    let [half, roof, spring] = [b[5], b[6], b[9]];
    let top = if z > spring && roof > spring {
        let r = roof - spring;
        (libm::hypot(left / half, (z - spring) / r) - 1.0) * half.min(r)
    } else {
        z - roof
    };
    (left.abs() - half).max(top)
}

/// How far a world point lies outside a box, in about metres; negative inside.
pub fn outside(b: &[f64; BOX], p: [f64; 3]) -> f64 {
    let (along, left) = box_frame(b, [p[0], p[1]]);
    outside_plan(b, [p[0], p[1]]).max(b[7] - (p[2] - risen(b, along, left))).max(outside_wall(b, p))
}
