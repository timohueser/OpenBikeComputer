//! The check of the rooms (Rooms in the spec): two rooms that draw and still come nearer than a wall on their final
//! floors, halves and heights.

use std::collections::BTreeMap;

use super::super::floor::Floor;
use super::super::{Curve, Input, Point, Tube};
use super::axes::{Axes, Foot};
use super::{Rooms, MERGE_M};

/// Two spaces lie apart: their gap is a wall or more, within the bake's centimetre, or one lies clear over the other,
/// a crown's shell and a slab over its crown; `rise` is the height of the second's floor over the first's.
fn apart((gap, rise): (f64, f64), (ha, hb): (f64, f64), input: &Input) -> bool {
    let b = &input.rules.bake;
    let (wall, roof) = (2.0 * b.shell_m, b.shell_m + b.slab_m);
    gap >= wall - MERGE_M || rise >= ha + roof || -rise >= hb + roof
}

/// The gap and the rise of tube b at its foot from metres s of tube a's axis: the metres between their spaces (each
/// tube's half to each side of its axis) and the height of b's floor over a's.
fn beside(tubes: &[Tube], (a, s): (usize, f64), foot: &Foot) -> (f64, f64) {
    let q = tubes[a].curve.at(s).0;
    (foot.d - tubes[a].half - tubes[foot.tube].half, foot.z - q.z)
}

/// Per two rooms that draw (`built`) and come beside each other (`beside` and `apart`), the first place where they do:
/// each room's centre line read every metre against the axes near it, on the final floors, halves and heights.
pub(crate) fn touching(
    rooms: &Rooms,
    (floors, built): (&[Option<Floor>], &[bool]),
    half: &[f64],
    input: &Input,
) -> Vec<[f64; 2]> {
    let wall = 2.0 * input.rules.bake.shell_m;
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
    let height = |t: usize, s: f64| floors[t].as_ref().map_or(0.0, |f| f.at(s + f.s[0]).1);
    // Two rooms of one seam meet on its line.
    let mut seam_of: Vec<Vec<usize>> = vec![Vec::new(); tubes.len()];
    for (s, seam) in rooms.seams.iter().enumerate() {
        for &(r, _) in [&seam.parent].into_iter().chain(&seam.children) {
            seam_of[r].push(s);
        }
    }
    let meet = |a: usize, b: usize| seam_of[a].iter().any(|s| seam_of[b].contains(s));
    let mut out: BTreeMap<(usize, usize), [f64; 2]> = BTreeMap::new();
    for (a, t) in tubes.iter().enumerate().filter(|(_, t)| !t.curve.stretches.is_empty()) {
        let steps = t.curve.len.ceil().max(1.0) as usize;
        for k in 0..=steps {
            let s = t.curve.len * k as f64 / steps as f64;
            let p = t.curve.at(s).0.p;
            for f in
                axes.near(p, a, t.half + widest + wall).iter().filter(|f| !f.past && f.tube > a && !meet(a, f.tube))
            {
                if !apart(beside(&tubes, (a, s), f), (height(a, s), height(f.tube, f.s)), input) {
                    out.entry((a, f.tube)).or_insert(p);
                }
            }
        }
    }
    out.into_values().collect()
}
