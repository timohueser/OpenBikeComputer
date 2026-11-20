//! The boxes of a part, in which the ground does not draw. Together they hold the room of the opening and of the tube,
//! and each lies inside the part's wall, at its middle; and how far a part runs into its tube.

use crate::mesh::{left_of, Station};
use crate::tube::{direction, spring, Meets, Net};
use crate::{metres_per_unit, Section};

/// Words of a box: the middle of its start (world units), its unit way in, its depth and its half-width (m), the
/// heights of its roof, its floor and its spring at its start, their rise per metre in, and the slopes of its start
/// and of its end: a point `c` metres to the left of its axis lies in the box from `start · c` metres in to `depth +
/// end · c`. Over its spring the roof is round, a half-ellipse from the half-width to the roof; where the spring is
/// the roof the box is flat.
pub const BOX: usize = 12;

/// How far end e's part runs in from its frame line: its boxes, and its shell, which ends with a face (`closed`) or
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
/// collar.
fn need(net: &Net, e: usize) -> f64 {
    let (end, design) = (&net.ends[e], &net.rules.design);
    let cell = design.mesh_texels * 32.0 * metres_per_unit(end.at[1]);
    (design.portal_cells * cell).max(end.collar + 1.0) + std::f64::consts::SQRT_2 * cell + 1.0
}

/// The metres in from end e's opening of each vertex of its walk to the other end of its tube: 0, then the far vertex
/// of each chord.
fn stops(net: &Net, e: usize) -> Vec<f64> {
    let walk = net.walk(e, f64::INFINITY);
    let mut out = vec![0.0];
    out.extend(walk.iter().map(|&(c, _, start)| start + direction(&net.chords[c]).1));
    out
}

/// The first stop at `metres` or past it; the last stop where none is.
fn at_or_past(stops: &[f64], metres: f64) -> f64 {
    stops.iter().copied().find(|&s| s >= metres).unwrap_or(stops[stops.len() - 1])
}

/// The reach of built end e. Its part runs to the first vertex of its tube at `need` or past it. Where the parts of the
/// tube's two ends would meet, each shell ends at the vertex nearest the middle, and there the taller shell closes the
/// step to the lower one; else a face closes each shell.
pub fn reach(net: &Net, e: usize) -> Reach {
    let walk = net.walk(e, f64::INFINITY);
    let at = stops(net, e);
    let (total, own) = (at[at.len() - 1], at_or_past(&at, need(net, e)));
    let other = walk.last().and_then(|&(c, forward, _)| net.end_at(c, usize::from(forward)));
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
    // A tube of one chord has no vertex between its openings: each shell runs to its own reach.
    let Some(mid) = mid else { return alone };
    let (here, there) = (&net.chords[walk[mid - 1].0], &net.chords[walk[mid].0]);
    let lower = (there.height < here.height && there.section == here.section).then_some((there.height, there.section));
    let foot = Some(net.ends[o].z + net.ends[o].foot);
    Reach { boxes: own, shell: at[mid], closed: false, then: Meets { foot, lower } }
}

/// The slope of a plane through a point, in the vertical plane of station `ring` (across it), as the slope of a box
/// end along the unit direction `u`: `−(left · n) / (u · n)` for the plane's horizontal normal n.
fn slope(u: [f64; 2], ring: &Station) -> f64 {
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
/// changes, and between them each point farther than `tol` metres from the chord of the piece (Douglas-Peucker in
/// metres, the floor included).
fn pieces(axis: &[Axis], cut: &[bool], tol: f64, m: f64) -> Vec<usize> {
    fn split(axis: &[Axis], a: usize, b: usize, tol: f64, m: f64, out: &mut Vec<usize>) {
        let q = |k: usize| [axis[k].p[0] * m, axis[k].p[1] * m, axis[k].z];
        let (pa, pb) = (q(a), q(b));
        let d = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
        let l2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
        let off = |k: usize| {
            let p = q(k);
            let w = [p[0] - pa[0], p[1] - pa[1], p[2] - pa[2]];
            let t = if l2 > 0.0 { ((w[0] * d[0] + w[1] * d[1] + w[2] * d[2]) / l2).clamp(0.0, 1.0) } else { 0.0 };
            libm::hypot(libm::hypot(w[0] - t * d[0], w[1] - t * d[1]), w[2] - t * d[2])
        };
        let worst = (a + 1..b).max_by(|&i, &j| off(i).total_cmp(&off(j)));
        if let Some(k) = worst.filter(|&k| off(k) > tol) {
            split(axis, a, k, tol, m, out);
            out.push(k);
            split(axis, k, b, tol, m, out);
        }
    }
    let mut out = vec![0];
    let mut a = 0;
    for b in (1..axis.len()).filter(|&b| b + 1 == axis.len() || cut[b]) {
        split(axis, a, b, tol, m, &mut out);
        out.push(b);
        a = b;
    }
    out
}

/// The boxes of built end e: the opening's, from the front of the head wall to the frame line, its outline grown to
/// the middle of the wall round it; then one per straight piece of the tube's axis out to its reach, each at the
/// middle of the wall of its section, its ends in the planes of the tube's rings there.
pub fn boxes(net: &Net, e: usize) -> Vec<[f64; BOX]> {
    let Some((c0, _)) = net.opening(e) else { return Vec::new() };
    let (end, design, bake) = (&net.ends[e], &net.rules.design, &net.rules.ladder.bake);
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
    let mut out = vec![[
        front.x,
        front.y,
        into[0],
        into[1],
        design.recess_m,
        half,
        roof,
        front.z + lift,
        rise,
        spring_at,
        0.0,
        0.0,
    ]];
    // The axis from the frame line: per chord of the walk its far vertex, with the height and section that end there.
    let reach = reach(net, e).boxes;
    let mut axis = vec![Axis { p: end.at, z: end.z, ring }];
    let mut sections = vec![(first.height, first.section, first.half)];
    for (c, forward, start) in net.walk(e, reach) {
        let chord = &net.chords[c];
        let v = if forward { chord.b } else { chord.a };
        axis.push(Axis { p: [v[0], v[1]], z: v[2], ring: net.ring_along(c, usize::from(forward), forward) });
        sections.push((chord.height, chord.section, chord.half));
        if start + direction(chord).1 >= reach {
            break;
        }
    }
    // A section changes at a vertex where the chords before and after it differ.
    let cut: Vec<bool> =
        (0..axis.len()).map(|k| k > 0 && k + 1 < axis.len() && sections[k] != sections[k + 1]).collect();
    let m = metres_per_unit(end.at[1]);
    let keep = pieces(&axis, &cut, bake.shell_m / 4.0, m);
    for w in keep.windows(2) {
        let (a, b) = (&axis[w[0]], &axis[w[1]]);
        let (h, section, half) = sections[w[0] + 1];
        let d = [b.p[0] - a.p[0], b.p[1] - a.p[1]];
        let l = libm::hypot(d[0], d[1]).max(f64::MIN_POSITIVE);
        let (u, depth) = ([d[0] / l, d[1] / l], l * m);
        let rise = (b.z - a.z) / depth.max(f64::MIN_POSITIVE);
        // The floors of the vertices between against the piece's line.
        let off = (w[0] + 1..w[1]).map(|k| {
            let q = &axis[k];
            let along = ((q.p[0] - a.p[0]) * u[0] + (q.p[1] - a.p[1]) * u[1]) * m;
            q.z - (a.z + rise * along)
        });
        let (low, high) = off.fold((0.0f64, 0.0f64), |(lo, hi), x| (lo.min(x), hi.max(x)));
        let mid = a.z + (low + high) / 2.0;
        // A ring that is not square to the piece tilts the floor across it: the floor of the room lies that much
        // under the piece's line at its side.
        let (start, end) = (slope(u, &a.ring), slope(u, &b.ring));
        let low = low - rise.abs() * half * start.abs().max(end.abs());
        let sp = spring(net, section, h);
        let (roof, spring_at) = roof_of(mid, h, sp, section, wall);
        out.push([a.p[0], a.p[1], u[0], u[1], depth, half + wall, roof, a.z + low + lift, rise, spring_at, start, end]);
    }
    out
}

/// How far a world point lies outside a box, in about metres; negative inside.
pub fn outside(b: &[f64; BOX], p: [f64; 3]) -> f64 {
    let m = metres_per_unit(b[1]);
    let d = [(p[0] - b[0]) * m, (p[1] - b[1]) * m];
    let lu = left_of([b[2], b[3]]);
    let (along, left) = (d[0] * b[2] + d[1] * b[3], d[0] * lu[0] + d[1] * lu[1]);
    let z = p[2] - b[8] * along;
    let [depth, half, roof, floor, spring] = [b[4], b[5], b[6], b[7], b[9]];
    let plan = (b[10] * left - along).max(along - depth - b[11] * left).max(left.abs() - half);
    let top = if z > spring && roof > spring {
        let r = roof - spring;
        (libm::hypot(left / half, (z - spring) / r) - 1.0) * half.min(r)
    } else {
        z - roof
    };
    plan.max(floor - z).max(top)
}
