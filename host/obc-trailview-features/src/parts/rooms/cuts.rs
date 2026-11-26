//! The rooms of the runs (Rooms in the spec): each arm cut where a run ends in the hill and where a lane that turned
//! away is a room again, a cut inside a run cutting the other arm too; the stretches between the cuts are lanes, those
//! that a run joins one room; and the seams where the rooms of one place meet.

use std::collections::BTreeMap;

use super::super::{along_dir, left_of, Input, Tube};
use super::runs::{End, Run};
use super::section::{crossing, foot};
use super::{Bound, Lane, Seam};

/// A room and one of its ends.
type RoomEnd = (usize, usize);

/// Per arm, its opening at each of its ends.
type Ends = [Option<usize>; 2];

/// What `cut` gives: per room its lanes (the base first) and its two ends, the seams, the stretches of arms in no room
/// (per stretch its arm and metres), and the places where neither side of a cut is one room.
pub(super) struct Cut {
    pub rooms: Vec<(Vec<Lane>, [Bound; 2])>,
    pub seams: Vec<Seam>,
    pub walls: Vec<(usize, [f64; 2])>,
    pub no_parent: Vec<[f64; 2]>,
}

/// The rooms of the runs over the arms from opening to opening (`spans`, with the opening at each end `at_end`). Each
/// arm is cut where a run ends in the hill, and at the metres `turned` (per lane that turned away: its arm, its metres
/// at the turn and where it is a room again); a cut inside a run cuts the other arm at its foot, until no cut is new,
/// and a cut within `vertex_gap_m` of another is that cut. The stretches between the cuts are lanes, and the lanes
/// that a run joins are one room; a stretch of a lane that turned away that no run joins is in no room. Where one side
/// of the cuts of one place is one room, that place is a seam: a line square to that room's base through its cut; its
/// lanes and those of the rooms on the other side end on the line. With no room on the other side it is a cap.
pub(super) fn cut(
    arms: &[Tube],
    (spans, at_end): (&[Option<[f64; 2]>], &[Ends]),
    runs: &[Run],
    turned: &[(usize, f64, f64)],
    input: &Input,
) -> Cut {
    let b = &input.rules.bake;
    let n = arms.len();
    let mut cuts: Vec<Vec<f64>> = vec![Vec::new(); n];
    let add = |cuts: &mut Vec<Vec<f64>>, t: usize, s: f64| {
        let [s0, s1] = spans[t].expect("an arm of a run");
        let new = s > s0 + 1e-6 && s < s1 - 1e-6 && !cuts[t].iter().any(|x| (x - s).abs() < b.vertex_gap_m);
        if new {
            cuts[t].push(s);
        }
        new
    };
    for r in runs {
        for side in (0..2).filter(|&side| r.ends[side] != End::Opens) {
            add(&mut cuts, r.a, r.sa[side]);
            add(&mut cuts, r.b, r.sb[side]);
        }
    }
    for &(t, _, again) in turned {
        add(&mut cuts, t, again);
    }
    // A cut inside a run cuts the other arm at its foot, until no cut is new. This ends: a new cut lies `vertex_gap_m`
    // or more from each cut of its arm, so an arm holds at most its length over that.
    let inside = |x: f64, [p, q]: [f64; 2]| x > p.min(q) + 1e-6 && x < p.max(q) - 1e-6;
    let along =
        |x: f64, (from, to): ([f64; 2], [f64; 2])| to[0] + (to[1] - to[0]) * (x - from[0]) / (from[1] - from[0]);
    loop {
        let mut new = false;
        for r in runs {
            for x in cuts[r.a].clone().into_iter().filter(|&x| inside(x, r.sa)) {
                if let Some(y) = foot(arms, (r.a, x), r.b, along(x, (r.sa, r.sb))) {
                    new |= add(&mut cuts, r.b, y);
                }
            }
            for y in cuts[r.b].clone().into_iter().filter(|&y| inside(y, r.sb)) {
                if let Some(x) = foot(arms, (r.b, y), r.a, along(y, (r.sb, r.sa))) {
                    new |= add(&mut cuts, r.a, x);
                }
            }
        }
        if !new {
            break;
        }
    }
    // The stretches between the cuts, and the sets of them that runs join.
    let mut stretches: Vec<(usize, [f64; 2])> = Vec::new();
    let mut first = vec![0; n + 1];
    for t in 0..n {
        first[t] = stretches.len();
        let Some([s0, s1]) = spans[t] else { continue };
        let mut at = cuts[t].clone();
        at.sort_by(f64::total_cmp);
        let points: Vec<f64> = [s0].into_iter().chain(at).chain([s1]).collect();
        stretches.extend(points.windows(2).map(|w| (t, [w[0], w[1]])));
    }
    first[n] = stretches.len();
    let of = |t: usize, s: f64| {
        (first[t]..first[t + 1]).find(|&k| s >= stretches[k].1[0] - 1e-6 && s <= stretches[k].1[1] + 1e-6)
    };
    let mut root: Vec<usize> = (0..stretches.len()).collect();
    let find = |root: &mut Vec<usize>, mut i: usize| {
        while root[i] != i {
            root[i] = root[root[i]];
            i = root[i];
        }
        i
    };
    for r in runs {
        for (k, (_, [s0, s1])) in stretches.iter().enumerate().take(first[r.a + 1]).skip(first[r.a]) {
            let m = (s0 + s1) / 2.0;
            if !inside(m, r.sa) {
                continue;
            }
            let Some(other) = foot(arms, (r.a, m), r.b, along(m, (r.sa, r.sb))).and_then(|y| of(r.b, y)) else {
                continue;
            };
            let (x, y) = (find(&mut root, k), find(&mut root, other));
            root[x.max(y)] = x.min(y);
        }
    }
    let mut sets: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for k in 0..stretches.len() {
        let r = find(&mut root, k);
        sets.entry(r).or_default().push(k);
    }
    // Per room its lanes, the base (the widest arm, then the lowest id) running the room's way, and its two ends.
    let walled = |k: usize| {
        let (t, [s0, s1]) = stretches[k];
        let m = (s0 + s1) / 2.0;
        turned.iter().any(|&(w, x, again)| w == t && m > x.min(again) && m < x.max(again))
    };
    let mut rooms: Vec<(Vec<Lane>, [Bound; 2])> = Vec::new();
    let mut walls = Vec::new();
    let mut cut_ends: BTreeMap<(usize, u64), Vec<RoomEnd>> = BTreeMap::new();
    for set in sets.into_values() {
        if let [k] = set[..] {
            if walled(k) {
                walls.push(stretches[k]);
                continue;
            }
        }
        let base = *set
            .iter()
            .max_by(|&&x, &&y| {
                let (tx, ty) = (&arms[stretches[x].0], &arms[stretches[y].0]);
                tx.half.total_cmp(&ty.half).then(ty.widest.cmp(&tx.widest))
            })
            .unwrap();
        let start = arms[stretches[base].0].curve.at(stretches[base].1[0]).0.p;
        let lanes: Vec<Lane> = set
            .iter()
            .map(|&k| {
                let (t, [s0, s1]) = stretches[k];
                let (p0, p1) = (arms[t].curve.at(s0).0.p, arms[t].curve.at(s1).0.p);
                let forward = k == base || super::super::distance(p0, start) <= super::super::distance(p1, start);
                let span = if forward { [s0, s1] } else { [s1, s0] };
                let [a0, a1] = spans[t].unwrap();
                let side = |s: f64| [a0, a1].iter().position(|&e| (s - e).abs() < 1e-6);
                Lane { arm: t, span, ends: span.map(|s| side(s).and_then(|e| at_end[t][e])) }
            })
            .collect();
        let r = rooms.len();
        let bounds = [0, 1].map(|end| {
            // Per lane: at an opening, at a cut, or at an end with no opening.
            let at = |l: &Lane| {
                let [s0, s1] = spans[l.arm].unwrap();
                let at_span = (l.span[end] - s0).abs() < 1e-6 || (l.span[end] - s1).abs() < 1e-6;
                if at_span {
                    l.ends[end].map(|_| true)
                } else {
                    Some(false)
                }
            };
            let kinds: Vec<Option<bool>> = lanes.iter().map(at).collect();
            if kinds.iter().all(|k| *k == Some(true)) {
                Bound::Open
            } else if kinds.iter().all(|k| *k == Some(false)) {
                for l in &lanes {
                    cut_ends.entry((l.arm, l.span[end].to_bits())).or_default().push((r, end));
                }
                Bound::Seam(usize::MAX)
            } else {
                Bound::None
            }
        });
        let mut lanes = lanes;
        let base_at = set.iter().position(|&k| k == base).unwrap();
        lanes.swap(0, base_at);
        rooms.push((lanes, bounds));
    }
    // The room ends that share a cut are one place.
    let mut group: BTreeMap<RoomEnd, usize> = BTreeMap::new();
    let mut groups: Vec<Vec<RoomEnd>> = Vec::new();
    for list in cut_ends.values() {
        let known = list.iter().find_map(|e| group.get(e).copied());
        let g = known.unwrap_or_else(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        for &e in list {
            if let Some(&h) = group.get(&e).filter(|&&h| h != g) {
                for x in std::mem::take(&mut groups[h]) {
                    group.insert(x, g);
                    groups[g].push(x);
                }
            } else if let std::collections::btree_map::Entry::Vacant(v) = group.entry(e) {
                v.insert(g);
                groups[g].push(e);
            }
        }
    }
    let (mut seams, mut no_parent): (Vec<Seam>, Vec<[f64; 2]>) = (Vec::new(), Vec::new());
    for list in groups.into_iter().filter(|g| !g.is_empty()) {
        // The sides: a room lies after the cut where its stretch starts there, its arm led the way of the first room's
        // base there.
        let (r0, e0) = list[0];
        let l0 = &rooms[r0].0[0];
        let d = arms[l0.arm].curve.dir(l0.span[e0]);
        let side_of = |&(r, e): &RoomEnd| {
            let l = &rooms[r].0[0];
            let after = l.span[1 - e] > l.span[e];
            let own = arms[l.arm].curve.dir(l.span[e]);
            after == (own[0] * d[0] + own[1] * d[1] >= 0.0)
        };
        let (after, before): (Vec<RoomEnd>, Vec<RoomEnd>) = list.iter().partition(|e| side_of(e));
        let lanes = |x: &RoomEnd| rooms[x.0].0.len();
        let (parent, children) = if before.len() == 1 && (after.len() != 1 || lanes(&before[0]) >= lanes(&after[0])) {
            (before[0], after)
        } else if after.len() == 1 {
            (after[0], before)
        } else {
            no_parent.push(arms[l0.arm].curve.at(l0.span[e0]).0.p);
            for &(r, e) in &list {
                rooms[r].1[e] = Bound::None;
            }
            continue;
        };
        let base = &rooms[parent.0].0[0];
        let at = base.span[parent.1];
        let (p, d) = (arms[base.arm].curve.at(at).0.p, arms[base.arm].curve.dir(at));
        // The heading leads from the room before the line into the others, or out of it at a cap.
        let toward = arms[base.arm].curve.at(base.span[1 - parent.1]).0.p;
        let dir = if along_dir(p, toward, d) < 0.0 { d } else { [-d[0], -d[1]] };
        let s = seams.len();
        for &(r, e) in [&parent].into_iter().chain(&children) {
            rooms[r].1[e] = Bound::Seam(s);
            // Each lane ends on the line.
            for l in &mut rooms[r].0 {
                if let Some((x, _)) = crossing(&arms[l.arm], p, left_of(dir), l.span[e]) {
                    l.span[e] = x;
                }
            }
        }
        seams.push(Seam { parent, children, p, dir, holes: Vec::new(), doors: Vec::new(), whole: true });
    }
    Cut { rooms, seams, walls, no_parent }
}
