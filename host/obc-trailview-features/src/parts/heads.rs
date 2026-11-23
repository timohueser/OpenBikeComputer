//! The check of the heads (Tunnel parts in the spec): a head, with the shells of its collars down to its foot, stands
//! in no other room that draws; and how far its openings move in along their ways so that it clears.

use std::collections::BTreeSet;

use super::ladder::{Decided, Hole, Kind, Numbers};
use super::rooms::{Drawn, Rooms};
use super::{distance, first_way_m, left_of, offset, Input, Opening};
use crate::metres;

/// A head that stands in another room: its record's opening, the place, and the least whole metres more that its
/// openings move in so that it clears; None where no metre within their limits does.
pub(crate) struct Stand {
    pub opening: usize,
    pub at: [f64; 2],
    pub clear: Option<f64>,
}

/// A point of a part: in plan, and the heights of the part's bottom and top there.
type Sample = ([f64; 2], f64, f64);

/// A centimetre: a point lies in a room by more than this.
const MERGE_M: f64 = 0.01;

/// The heads that stand in another room that draws, on the final floors, halves and heights: a point of the part lies
/// inside the room's space (its outline swept along its axis, `half` to each side, from its floor to its arch) by more
/// than a centimetre. A part is its head wall from `-recess_m` to `face_m - recess_m` across its frame line, `half` +
/// `shell_m` (a rim: + `frame_m`) past its outer holes and its wing rows past that, from its foot to `face` (a rim: to
/// each hole's frame); and per hole the shell of its collar, `half` + `shell_m` to each side of its room's axis, from
/// the foot to `shell_m` over its crown, out to the first vertex of its floor at the builder's reach (`need` of
/// `obc-trailview-parts`), of a box to the middle of its room. The foot is the record's, or `foot_m` under the lowest
/// floor of those collars, as the builder's (`boxes::foot`). The rooms of a head's holes are its own, and the rooms
/// open to them through the hole of a seam.
pub(crate) fn standing(rooms: &Rooms, openings: &[Opening], decided: &Decided, input: &Input) -> Vec<Stand> {
    let b = &input.rules.bake;
    let spring = obc_trailview_parts::Rules::embedded().design.spring;
    let drawn = Drawn::new(rooms, (&decided.floors, &decided.draws), &decided.half);
    let widest = drawn.tubes.iter().map(|t| t.half).fold(0.0, f64::max);
    let mut boxed = vec![false; rooms.tubes.len()];
    for (o, d) in openings.iter().zip(&decided.ends) {
        boxed[o.tube] |= d.kind == Some(Kind::Box);
    }
    let boxed = rooms.one_section(boxed);
    let mut out = Vec::new();
    for (i, d) in decided.ends.iter().enumerate() {
        let (Some(kind), Some(n)) = (d.kind, &d.numbers) else { continue };
        let tube = |h: &Hole| openings[h.opening].tube;
        if n.holes.iter().any(|h| !decided.draws[tube(h)]) {
            continue;
        }
        let mut own: BTreeSet<usize> = n.holes.iter().map(tube).collect();
        for seam in &rooms.seams {
            let (parent, mut children) = (seam.parent.0, seam.children.iter().map(|c| c.0));
            if own.contains(&parent) {
                own.extend(children);
            } else if children.any(|r| own.contains(&r)) {
                own.insert(parent);
            }
        }
        let parts = samples(i, (kind, n), openings, decided, (rooms, input));
        // Whether a point of the part, moved by `by` world units, lies in another room: in its section and under the
        // room's arch over the point, its top on a round room's outline there.
        let stands = |by: [f64; 2]| {
            parts.iter().any(|&(p, bottom, top)| {
                let q = [p[0] + by[0], p[1] + by[1]];
                drawn.axes.near(q, usize::MAX, widest + 1.0).iter().any(|f| {
                    // The room's section at the foot of the point, and its arch over the point.
                    let sec = drawn.at(f.tube, f.s);
                    let (len, half) = (drawn.tubes[f.tube].curve.len, distance(sec[0], sec[1]) / 2.0);
                    let inside = f.past.is_none() && f.s > MERGE_M && f.s < len - MERGE_M && f.d < half - MERGE_M;
                    let h = drawn.height(f.tube, f.s);
                    let arch = (1.0 - (f.d / half).powi(2)).max(0.0).sqrt();
                    let share = if boxed[f.tube] { 1.0 } else { spring + (1.0 - spring) * arch };
                    !own.contains(&f.tube) && inside && bottom < f.z + h * share - MERGE_M && top > f.z + MERGE_M
                })
            })
        };
        if !stands([0.0, 0.0]) {
            continue;
        }
        // The head moves in along the room of its record's opening, as far as each of its lanes' arms may move.
        let o = &openings[i];
        let room = &rooms.tubes[o.tube];
        let sign = if o.end == 0 { 1.0 } else { -1.0 };
        let most = (n.holes.iter())
            .flat_map(|h| {
                let q = &openings[h.opening];
                rooms.lanes[q.tube].iter().filter_map(move |l| l.ends[q.end])
            })
            .map(|a| {
                let ao = &rooms.arm_openings[a];
                let arm = &rooms.arms[ao.tube];
                b.junction_shift_max_m.min(arm.curve.len / 2.0).min(first_way_m(arm, ao.end)) - ao.shift
            })
            .fold(f64::INFINITY, f64::min);
        let at = room.curve.at(o.s).0.p;
        let clear = (1..=most.max(0.0).floor() as usize).map(|m| m as f64).find(|&m| {
            let p = room.curve.at(o.s + sign * m).0.p;
            !stands([p[0] - at[0], p[1] - at[1]])
        });
        out.push(Stand { opening: i, at: o.p, clear });
    }
    out
}

/// The points of the part of the record at opening i, of the numbers `n`: of its head wall, and of the shell of each
/// hole's collar.
fn samples(
    i: usize,
    (kind, n): (Kind, &Numbers),
    openings: &[Opening],
    decided: &Decided,
    (rooms, input): (&Rooms, &Input),
) -> Vec<Sample> {
    let b = &input.rules.bake;
    let design = &obc_trailview_parts::Rules::embedded().design;
    let o = &openings[i];
    let kind_of = |h: &Hole| decided.ends[h.opening].kind.unwrap_or(kind);
    let rim = n.holes.iter().all(|h| matches!(kind_of(h), Kind::Rock | Kind::Free));
    let closed = |h: &Hole| kind_of(h) == Kind::Free && h.collar < 0.005;
    // A level-17 cell of the builder in metres: `mesh_texels` texels of 32 of its world units, 2^30 per side.
    let cell = design.mesh_texels * 32.0 / (1u64 << 30) as f64 * metres(o.p[1]);
    // Per hole its opening, its way into the room, the metres to the end of its collar and the lowest floor to there.
    let reach: Vec<(usize, f64, f64, f64)> = (n.holes.iter())
        .filter_map(|h| {
            let q = &openings[h.opening];
            let f = decided.floors[q.tube].as_ref()?;
            let (s0, s1) = (f.s[0], f.s[f.s.len() - 1]);
            let sign = if q.end == 0 { 1.0 } else { -1.0 };
            let need = if kind_of(h) == Kind::Box {
                (s1 - s0) / 2.0
            } else {
                (design.portal_cells * cell).max(h.collar + 1.0) + std::f64::consts::SQRT_2 * cell + 1.0
            };
            // The first vertex of the floor at `need` or past it, within the room.
            let past =
                (f.s.iter().map(|&s| sign * (s - q.s))).filter(|&m| m >= need - 1e-6).fold(f64::INFINITY, f64::min);
            let to = past.min(if sign > 0.0 { s1 - q.s } else { q.s - s0 });
            let low = (f.s.iter().zip(&f.f))
                .filter(|(&s, _)| (0.0..=to).contains(&(sign * (s - q.s))))
                .map(|(_, &z)| z)
                .fold(f.at(q.s).0.min(f.at(q.s + sign * to).0), f64::min);
            Some((h.opening, sign, to, low))
        })
        .collect();
    let low = reach.iter().map(|r| r.3).fold(f64::INFINITY, f64::min);
    let foot = (o.floor + n.foot).min(low - b.foot_m);
    let mut out: Vec<Sample> = Vec::new();
    // The head, where a hole opens.
    if n.holes.iter().any(|h| !closed(h)) {
        let d = [n.heading.cos(), n.heading.sin()];
        let l = left_of(d);
        let grow = b.shell_m + if rim { design.frame_m } else { 0.0 };
        let edge = |h: &Hole| decided.half[openings[h.opening].tube] + grow;
        let rows = if rim { (0.0, 0.0) } else { (n.right.len() as f64, n.left.len() as f64) };
        let lo = n.holes.iter().map(|h| h.across - edge(h)).fold(f64::INFINITY, f64::min) - rows.0;
        let hi = n.holes.iter().map(|h| h.across + edge(h)).fold(f64::NEG_INFINITY, f64::max) + rows.1;
        let top = if n.walls && !rim {
            o.floor + n.face
        } else {
            (n.holes.iter())
                .map(|h| openings[h.opening].floor + h.height + design.frame_m + b.shell_m)
                .fold(f64::NEG_INFINITY, f64::max)
        };
        let steps = ((hi - lo) / 0.5).ceil().max(1.0) as usize;
        for depth in [-design.recess_m, design.face_m - design.recess_m] {
            let line = offset(o.p, d, depth);
            out.extend((0..=steps).map(|k| (offset(line, l, lo + (hi - lo) * k as f64 / steps as f64), foot, top)));
        }
    }
    // The shell of each collar.
    for &(h, sign, to, _) in &reach {
        let q = &openings[h];
        let (room, f) = (&rooms.tubes[q.tube], decided.floors[q.tube].as_ref().expect("a room that draws"));
        let w = decided.half[q.tube] + b.shell_m;
        let across = (w / 0.5).ceil() as usize;
        for m in (0..=to.ceil() as usize).map(|m| (m as f64).min(to)) {
            let s = q.s + sign * m;
            let (p, l) = (room.curve.at(s).0.p, left_of(room.curve.dir(s)));
            let (z, height) = f.at(s);
            let crown = z + height + b.shell_m;
            out.extend((0..=2 * across).map(|k| (offset(p, l, -w + w * k as f64 / across as f64), foot, crown)));
        }
    }
    out
}
