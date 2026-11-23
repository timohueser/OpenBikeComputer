//! The check of the rooms (Rooms in the spec): two rooms that draw and still come nearer than a wall on their final
//! floors, halves and heights.

use std::collections::BTreeMap;

use super::super::floor::Floor;
use super::super::{along_dir, distance, left_of, offset, segment_m, Curve, Input, Point, Tube};
use super::axes::{Axes, Foot};
use super::{Bound, Rooms, MERGE_M};

/// Two spaces lie apart: their gap is a wall or more, or one lies clear over the other, a crown's shell and a slab over
/// its crown, each within the bake's centimetre; `rise` is the height of the second's floor over the first's.
fn apart((gap, rise): (f64, f64), (ha, hb): (f64, f64), input: &Input) -> bool {
    let b = &input.rules.bake;
    let (wall, roof) = (2.0 * b.shell_m, b.shell_m + b.slab_m);
    gap >= wall - MERGE_M || rise >= ha + roof - MERGE_M || -rise >= hb + roof - MERGE_M
}

/// The gap and the rise of tube b at its foot from metres s of tube a's axis: the metres between their spaces (each
/// tube's half to each side of its axis) and the height of b's floor over a's. Past b's end, b's space ends at the
/// face across its end: the gap is the metres from a's section there to that face.
fn beside(tubes: &[Tube], (a, s): (usize, f64), foot: &Foot) -> (f64, f64) {
    let (t, o) = (&tubes[a], &tubes[foot.tube]);
    let q = t.curve.at(s).0;
    let Some(e) = foot.past else { return (foot.d - t.half - o.half, foot.z - q.z) };
    let at = if e == 0 { 0.0 } else { o.curve.len };
    let (end, la, lb) = (o.curve.at(at).0.p, left_of(t.curve.dir(s)), left_of(o.curve.dir(at)));
    let section = |p: [f64; 2], l: [f64; 2], half: f64| [offset(p, l, -half), offset(p, l, half)];
    (segments_m(section(q.p, la, t.half), section(end, lb, o.half)), foot.z - q.z)
}

/// The metres between two segments, 0 where they cross.
fn segments_m([a, b]: [[f64; 2]; 2], [c, d]: [[f64; 2]; 2]) -> f64 {
    if super::super::measure::meet(a, b, c, d).is_some() {
        return 0.0;
    }
    [segment_m(a, c, d), segment_m(b, c, d), segment_m(c, a, b), segment_m(d, a, b)]
        .into_iter()
        .fold(f64::INFINITY, f64::min)
}

/// Per two rooms that draw (`built`) and come beside each other (`beside` and `apart`), the first place where they do:
/// each room's centre line read every metre against the axes near it, from both rooms, on the final floors, halves and
/// heights; a foot past the other's end is the end. Two rooms of one seam meet on its line: their metres from the line
/// to the first vertex of each room behind it (where that room tapers from its hole), and a foot past the end of one at
/// that seam, do not count.
pub(crate) fn touching(
    rooms: &Rooms,
    (floors, built): (&[Option<Floor>], &[bool]),
    half: &[f64],
    input: &Input,
) -> Vec<[f64; 2]> {
    let b = &input.rules.bake;
    let wall = 2.0 * b.shell_m;
    // The rooms that draw at their floors, with their halves; another room is none.
    let tubes: Vec<Tube> = (rooms.tubes.iter().zip(floors).zip(half).zip(built))
        .map(|(((t, f), &half), &built)| {
            let mut t = t.clone();
            let Some(f) = f.as_ref().filter(|_| built) else {
                t.curve.stretches.clear();
                return t;
            };
            let (s0, s1) = (f.s[0], f.s[f.s.len() - 1]);
            let points: Vec<Point> =
                f.s.iter().zip(&f.f).map(|(&s, &z)| Point { z, s: s - s0, ..t.curve.at(s).0 }).collect();
            t.curve = Curve { stretches: vec![(t.widest, points)], len: s1 - s0 };
            Tube { half, ..t }
        })
        .collect();
    let axes = Axes::new(&tubes);
    let widest = tubes.iter().map(|t| t.half).fold(0.0, f64::max);
    // A room's height as the builder draws it: of a room of lanes, between its vertices; of a lone tube, its chord's.
    let height = |t: usize, s: f64| {
        floors[t].as_ref().map_or(0.0, |f| {
            let s = s + f.s[0];
            let k = f.s.partition_point(|&x| x <= s).clamp(1, f.s.len() - 1);
            if rooms.lanes[t].len() > 1 {
                f.at(s).1
            } else {
                f.h[k - 1].min(f.h[k])
            }
        })
    };
    // The seams of each room.
    let mut seam_of: Vec<Vec<usize>> = vec![Vec::new(); tubes.len()];
    for (s, seam) in rooms.seams.iter().enumerate() {
        for &(r, _) in [&seam.parent].into_iter().chain(&seam.children) {
            seam_of[r].push(s);
        }
    }
    let shared = |a: usize, c: usize| seam_of[a].iter().copied().filter(|s| seam_of[c].contains(s)).collect::<Vec<_>>();
    // Per seam the metres from its line to the farthest first vertex of a room behind it.
    let taper: Vec<f64> = (rooms.seams.iter())
        .map(|seam| {
            let first = |&(r, e): &(usize, usize)| {
                let c = &rooms.tubes[r].curve;
                let ps: Vec<[f64; 2]> = c.stretches.iter().flat_map(|(_, ps)| ps.iter().map(|q| q.p)).collect();
                let k = if e == 0 { 1 } else { ps.len().saturating_sub(2) };
                ps.get(k).map_or(0.0, |&q| along_dir(seam.p, q, seam.dir).abs())
            };
            seam.children.iter().map(first).fold(b.vertex_gap_m, f64::max)
        })
        .collect();
    // Point p lies at seam s: between its line and its taper's end, and in the reach of the room before it.
    let at_seam = |p: [f64; 2], s: usize| {
        let seam = &rooms.seams[s];
        along_dir(seam.p, p, seam.dir).abs() <= taper[s] + MERGE_M
            && distance(seam.p, p) <= half[seam.parent.0] + wall + taper[s]
    };
    let mut out: BTreeMap<(usize, usize), [f64; 2]> = BTreeMap::new();
    for (a, t) in tubes.iter().enumerate().filter(|(_, t)| !t.curve.stretches.is_empty()) {
        let steps = t.curve.len.ceil().max(1.0) as usize;
        for k in 0..=steps {
            let s = t.curve.len * k as f64 / steps as f64;
            let p = t.curve.at(s).0.p;
            for f in axes.near(p, a, t.half + widest + wall) {
                let pair = (a.min(f.tube), a.max(f.tube));
                let seams = shared(a, f.tube);
                let past_seam =
                    f.past.is_some_and(|e| matches!(rooms.bounds[f.tube][e], Bound::Seam(s) if seams.contains(&s)));
                if out.contains_key(&pair) || past_seam || seams.iter().any(|&s| at_seam(p, s)) {
                    continue;
                }
                if !apart(beside(&tubes, (a, s), &f), (height(a, s), height(f.tube, f.s)), input) {
                    out.insert(pair, p);
                }
            }
        }
    }
    out.into_values().collect()
}
