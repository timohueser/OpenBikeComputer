//! Tunnel ways that come nearer than a wall are one room (Tunnel parts in the spec, Rooms): the metres of two arms side
//! by side and their runs (`runs`), the rooms of the runs and their seams (`cuts`), each room's centre line and section
//! (`section`), the holes of each seam (`holes`), and the check of the rooms that draw (`touch`).

mod axes;
mod cuts;
mod holes;
mod runs;
mod section;
mod touch;

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::ladder::Kind;
use super::{along_dir, distance, left_of, offset, Input, Opening, Tube};
use crate::degrees;
use holes::{holes_of, Holes, Start};
use runs::End;
use section::{crossing, room_of, started, stretch};

pub(crate) use axes::Axes;
pub(crate) use section::{hold, onto};
pub(crate) use touch::{apart, touching};

/// The bake's centimetre (`MERGE_M` of the floor pass).
const MERGE_M: f64 = 0.01;

/// A lane of a room: its arm, and per room end its metres along the arm and the opening of the arm there.
#[derive(Clone, Debug)]
pub(crate) struct Lane {
    pub arm: usize,
    pub span: [f64; 2],
    pub ends: [Option<usize>; 2],
}

/// A lane at a vertex of a room's centre line: its axis's metres to the left of the room's axis and its half-width,
/// both in the metres of the ring there (along the ring's line, times its cosine), and its metres along its arm.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Place {
    pub off: f64,
    pub half: f64,
    pub s: f64,
}

/// The rooms: per room its tube (of a room of one lane, its arm or the stretch of its arm), its lanes from the right
/// wall to the left, its section per vertex of its centre line (by the metres of the
/// vertex along the line) and its two ends; the seams; the arms, and their openings with those of the lanes moved onto
/// their rooms' frame lines; and the counts.
pub(crate) struct Rooms {
    pub tubes: Vec<Tube>,
    /// Of a room of one lane, the metres along its arm where its own metres start.
    pub from: Vec<f64>,
    pub lanes: Vec<Vec<Lane>>,
    pub table: Vec<Vec<(f64, Vec<Place>)>>,
    pub bounds: Vec<[Bound; 2]>,
    pub seams: Vec<Seam>,
    /// Per room whether the rooms alone keep it from drawing: a seam at one of its ends holds no hole for a room behind
    /// it, or an arm of it has an opening in no room.
    pub broken: Vec<bool>,
    /// Per stretch of two arms over one another: the room over, the room under, and whether only the section of the
    /// room under goes lower (else its floor too: they cross).
    pub over: Vec<(usize, usize, bool)>,
    pub arms: Vec<Tube>,
    pub arm_openings: Vec<Opening>,
    pub metrics: Value,
}

/// A way that enters a room's opening: its end vertex, its id and the metres along it to the frame line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Way {
    pub way: [i64; 2],
    pub id: u32,
    pub shift: f64,
}

impl Rooms {
    /// Whether end e of room t is a seam.
    pub fn seam_at(&self, t: usize, e: usize) -> bool {
        matches!(self.bounds[t][e], Bound::Seam(_))
    }

    /// Per room whether it draws as parts: it is not `out` nor `broken`, and each of its ends is a seam or of a built
    /// kind (`kind` of its opening). The rooms of one seam and the rooms of one arm draw together or not at all.
    pub fn built(&self, kind: &dyn Fn(usize, usize) -> Option<Kind>, out: &[bool]) -> Vec<bool> {
        let n = self.tubes.len();
        let own = |t: usize| {
            !out[t]
                && !self.broken[t]
                && (0..2).all(|e| self.seam_at(t, e) || obc_trailview_parts::built(&[kind(t, e)]))
        };
        let mut root: Vec<usize> = (0..n).collect();
        let find = |root: &mut Vec<usize>, mut i: usize| {
            while root[i] != i {
                root[i] = root[root[i]];
                i = root[i];
            }
            i
        };
        let mut join = |members: &mut dyn Iterator<Item = usize>| {
            let Some(first) = members.next() else { return };
            for m in members {
                let (a, b) = (find(&mut root, first), find(&mut root, m));
                root[a.max(b)] = a.min(b);
            }
        };
        for seam in &self.seams {
            join(&mut [seam.parent].iter().chain(&seam.children).map(|m| m.0));
        }
        let mut of_arm: Vec<Vec<usize>> = vec![Vec::new(); self.arms.len()];
        for (r, lanes) in self.lanes.iter().enumerate() {
            lanes.iter().for_each(|l| of_arm[l.arm].push(r));
        }
        for rooms in &of_arm {
            join(&mut rooms.iter().copied());
        }
        let mut ok = vec![true; n];
        for t in (0..n).filter(|&t| !own(t)) {
            let r = find(&mut root, t);
            ok[r] = false;
        }
        (0..n).map(|t| ok[find(&mut root, t)]).collect()
    }

    /// The places of room r's lanes at metres s along its centre line, linear between the vertices of its table.
    pub fn places(&self, r: usize, s: f64) -> Vec<Place> {
        let table = &self.table[r];
        let k = table.partition_point(|(m, _)| *m <= s).clamp(1, table.len() - 1);
        let ((m0, a), (m1, z)) = (&table[k - 1], &table[k]);
        let u = ((s - m0) / (m1 - m0).max(f64::MIN_POSITIVE)).clamp(0.0, 1.0);
        let mix = |x: f64, y: f64| x + (y - x) * u;
        (a.iter().zip(z))
            .map(|(p, q)| Place { off: mix(p.off, q.off), half: mix(p.half, q.half), s: mix(p.s, q.s) })
            .collect()
    }

    /// The ways that enter opening o of a room, from the right of the way in: of a lone arm, its own; of a room of lanes,
    /// each lane's way moved in along it onto o's frame line.
    pub fn ways(&self, o: &Opening) -> Vec<Way> {
        if self.lanes[o.tube].len() < 2 {
            return vec![Way { way: o.way, id: o.way_id, shift: o.shift }];
        }
        let lanes = &self.lanes[o.tube];
        let mut out: Vec<(f64, Way)> = lanes
            .iter()
            .filter_map(|l| l.ends[o.end])
            .zip(lanes)
            .map(|(j, l)| {
                let (arm, open) = (&self.arms[l.arm], &self.arm_openings[j]);
                let shift = onto(arm, open, (o.p, o.dir), f64::INFINITY).unwrap_or(open.shift);
                let at = open.at(arm, shift).p;
                (along_dir(o.p, at, left_of(o.dir)), Way { way: open.way, id: open.way_id, shift })
            })
            .collect();
        out.sort_by(|x, y| x.0.total_cmp(&y.0));
        out.into_iter().map(|(_, w)| w).collect()
    }
}

/// A seam: the room before the line and its end there, the rooms behind it with their ends, the line's point (on the
/// base of the room before) and its heading from that room into the others; per room behind the line its hole's metres
/// across from the end of the room before and its half-width along the line; per lane of the room before that ends on
/// the line its door's metres across, half-width and arm; and whether the row of the room before holds a hole for each
/// room behind it. With no room behind it, a seam is a cap.
#[derive(Clone, Debug)]
pub(crate) struct Seam {
    pub parent: (usize, usize),
    pub children: Vec<(usize, usize)>,
    pub p: [f64; 2],
    pub dir: [f64; 2],
    pub holes: Vec<(f64, f64)>,
    pub doors: Vec<(f64, f64, usize)>,
    pub whole: bool,
}

/// An end of a room: of its lanes' openings, at a seam's line, or none (a lane ends where its tunnel goes on with no
/// opening, or the lanes end in other ways).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Bound {
    Open,
    Seam(usize),
    None,
}

/// A room as `rooms` makes it: its tube, the metres along its arm where a room of one lane starts, its lanes, its
/// section per vertex, its openings and its two ends.
#[derive(Clone)]
struct Made {
    tube: Tube,
    from: f64,
    lanes: Vec<Lane>,
    table: Vec<(f64, Vec<Place>)>,
    openings: Vec<Opening>,
    bounds: [Bound; 2],
}

/// The rooms of the arms (`cuts`), from the runs of two arms (`runs`). A run ends where an arm opens, at the openings of
/// both arms on that side; else in the hill at its last metre, a seam. Where the next metres are across, of its two
/// arms the narrower (then the higher id) turns away: its lane ends on the seam's line with a door, and its arm is in
/// no room until the first metre `vertex_gap_m` or more past it where it is with or across no arm; there its room starts
/// with a cap. A seam's holes come from the row of the room before it (`holes`). The lanes of a room open on one frame
/// line at each end with openings (`section`).
pub(crate) fn rooms(arms: Vec<Tube>, arm_openings: Vec<Opening>, input: &Input) -> (Rooms, Vec<Opening>) {
    let b = &input.rules.bake;
    let n = arms.len();
    // Per arm its openings at its two ends, and its metres from opening to opening.
    let mut at_end: Vec<[Option<usize>; 2]> = vec![[None; 2]; n];
    for (i, o) in arm_openings.iter().enumerate() {
        at_end[o.tube][o.end].get_or_insert(i);
    }
    let spans: Vec<Option<[f64; 2]>> = (0..n)
        .map(|t| {
            let s0 = at_end[t][0].map_or(0.0, |i| arm_openings[i].s);
            let s1 = at_end[t][1].map_or(arms[t].curve.len, |i| arm_openings[i].s);
            Some([s0, s1])
        })
        .collect();
    // An arm runs beside others where it can draw: it opens at both ends, its heights are known and it has a length.
    let live: Vec<Option<[f64; 2]>> = (0..n)
        .map(|t| spans[t].filter(|s| at_end[t].iter().all(Option::is_some) && arms[t].heights && s[1] - s[0] > 1e-6))
        .collect();
    let metres = runs::metres(&arms, &live, input);
    let (found, over_arms, short) = runs::runs(&metres, &live, input);
    let turned = runs::turned(&found, &metres, (&arms, &live), input);
    let cuts::Cut { mut rooms, seams, walls, no_parent } = cuts::cut(&arms, (&spans, &at_end), &found, &turned, input);
    let place = |p: [f64; 2]| {
        let [lat, lon] = degrees(p);
        format!("{lat:.5},{lon:.5}")
    };
    // The rooms' tubes, tables and openings. A room of one lane is its arm, or the stretch of its arm.
    let mut out = Rooms {
        tubes: Vec::new(),
        from: Vec::new(),
        lanes: Vec::new(),
        table: Vec::new(),
        bounds: Vec::new(),
        seams,
        broken: Vec::new(),
        over: Vec::new(),
        arms: Vec::new(),
        arm_openings: Vec::new(),
        metrics: Value::Null,
    };
    let mut openings: Vec<Opening> = Vec::new();
    let (mut off_line, mut moves): (Vec<String>, Vec<(f64, [f64; 2])>) = (Vec::new(), Vec::new());
    let mut moved_openings = arm_openings.clone();
    // A room of one lane at a seam ends on the seam's line, where its arm crosses it: there the room before the line
    // ends, and its hole lies.
    for seam in &out.seams {
        for &(r, e) in seam.children.iter().chain([&seam.parent]) {
            if let [l] = &mut rooms[r].0[..] {
                let [s0, s1] = spans[l.arm].unwrap();
                let cut = crossing(&arms[l.arm], seam.p, left_of(seam.dir), l.span[e]).map(|c| c.0);
                if let Some(s) = cut.filter(|&s| s >= s0 && s <= s1 && (s - l.span[1 - e]).abs() > b.vertex_gap_m) {
                    l.span[e] = s;
                }
            }
        }
    }
    // Each room after the room before each seam at which it lies behind the line: a seam's holes come from the row of
    // the room before it, and a room behind the line starts at its hole. A room of one lane is its arm, or the stretch
    // of its arm from its metres there on.
    let parents = |r: usize| {
        let seams = out.seams.iter().filter(move |s| s.children.iter().any(|c| c.0 == r));
        seams.map(|s| s.parent.0).collect::<Vec<usize>>()
    };
    let mut order: Vec<usize> = Vec::new();
    let mut done = vec![false; rooms.len()];
    while order.len() < rooms.len() {
        let ready = |r: usize| !done[r] && parents(r).iter().all(|&p| done[p] || p == r);
        let next = (0..rooms.len()).find(|&r| ready(r)).or_else(|| (0..rooms.len()).find(|&r| !done[r])).unwrap();
        done[next] = true;
        order.push(next);
    }
    let heading = |d: [f64; 2]| (d[1].atan2(d[0]) * 1000.0).round() / 1000.0;
    let clear_of = |sideways: f64| (sideways / b.pair_max_deg.to_radians().tan()).max(b.vertex_gap_m);
    let mut starts: BTreeMap<(usize, usize), Start> = BTreeMap::new();
    let mut made: Vec<Option<Made>> = vec![None; rooms.len()];
    for &r in &order {
        let (lanes, mut bounds) = rooms[r].clone();
        let start = [0, 1].map(|e| starts.get(&(r, e)));
        let room = if lanes.len() > 1 {
            let (tube, ordered, table, ends, moved, off) =
                room_of(r, &lanes, (&bounds, &out.seams, start), &arms, &moved_openings, input);
            for (i, o, m) in moved {
                moves.push((m, moved_openings[i].p));
                moved_openings[i] = o;
            }
            off_line.extend(off.into_iter().map(place));
            let mut opens = Vec::new();
            for (end, o) in ends.into_iter().enumerate() {
                match o {
                    Some(mut o) => {
                        (o.tube, o.end) = (r, end);
                        opens.push(o);
                    }
                    // A lane whose way does not reach the line: the room has no opening there.
                    None if bounds[end] == Bound::Open => bounds[end] = Bound::None,
                    None => {}
                }
            }
            Made { tube, from: 0.0, lanes: ordered, table, openings: opens, bounds }
        } else {
            let l = &lanes[0];
            let t = &arms[l.arm];
            let [s0, s1] = spans[l.arm].unwrap();
            let whole = (l.span[0] - s0).abs() < 1e-6 && (l.span[1] - s1).abs() < 1e-6;
            let (mut tube, from) = if whole { (t.clone(), 0.0) } else { (stretch(t, l.span), l.span[0]) };
            for (e, st) in start.iter().enumerate() {
                if let Some(st) = st {
                    let own = tube.curve.at(if e == 0 { 0.0 } else { tube.curve.len }).0.p;
                    started(&mut tube, e, st.p, clear_of(distance(st.p, own)));
                }
            }
            let opens = (l.ends.iter().flatten()).map(|&i| Opening {
                tube: r,
                s: arm_openings[i].s - from,
                ..arm_openings[i].clone()
            });
            Made { tube, from, lanes: vec![l.clone()], table: Vec::new(), openings: opens.collect(), bounds }
        };
        // The holes of the seams before which this room ends, from its row there.
        let before: Vec<usize> = (0..out.seams.len()).filter(|&s| out.seams[s].parent.0 == r).collect();
        for s in before {
            let seam = out.seams[s].clone();
            let e = seam.parent.1;
            let at = room.tube.curve.at(if e == 0 { 0.0 } else { room.tube.curve.len }).0.p;
            // Across to the left of the heading: the room's own left at its end 1, its right at its end 0.
            let sign = if e == 1 { 1.0 } else { -1.0 };
            let row: Vec<(usize, f64, f64, f64)> = if room.lanes.len() < 2 {
                vec![(room.lanes[0].arm, 0.0, room.tube.half, room.lanes[0].span[e])]
            } else {
                let (_, places) = &room.table[if e == 0 { 0 } else { room.table.len() - 1 }];
                (room.lanes.iter().zip(places)).map(|(l, p)| (l.arm, sign * p.off, p.half, p.s)).collect()
            };
            let children: Vec<Vec<usize>> =
                seam.children.iter().map(|&(c, _)| rooms[c].0.iter().map(|l| l.arm).collect()).collect();
            let h = heading(seam.dir);
            match holes_of(&row, &children, b.shell_m).filter(|_| !children.is_empty()) {
                Some(Holes { holes, doors }) => {
                    for (&child, (across, half, mut st)) in seam.children.iter().zip(holes) {
                        st.p = offset(at, left_of([h.cos(), h.sin()]), across);
                        starts.insert(child, st);
                        out.seams[s].holes.push((across, half));
                    }
                    out.seams[s].doors = doors;
                }
                None => out.seams[s].whole = children.is_empty(),
            }
        }
        made[r] = Some(room);
    }
    for room in made.into_iter().map(|m| m.expect("each room is made")) {
        openings.extend(room.openings);
        out.tubes.push(room.tube);
        out.from.push(room.from);
        out.lanes.push(room.lanes);
        out.table.push(room.table);
        out.bounds.push(room.bounds);
    }
    // A room behind a seam made before the room in front of it has no hole.
    for s in 0..out.seams.len() {
        out.seams[s].whole &= out.seams[s].holes.len() == out.seams[s].children.len();
    }
    // A room behind a seam made before the room in front of it has no hole.
    for s in 0..out.seams.len() {
        out.seams[s].whole &= out.seams[s].holes.len() == out.seams[s].children.len();
    }
    // A room cannot draw where a seam at its ends is not whole, or an arm of it has an opening in no room.
    let mut opened = vec![false; arm_openings.len()];
    for l in out.lanes.iter().flatten() {
        l.ends.iter().flatten().for_each(|&i| opened[i] = true);
    }
    out.broken = (0..out.tubes.len())
        .map(|r| {
            let seam = (0..2).any(|e| matches!(out.bounds[r][e], Bound::Seam(s) if !out.seams[s].whole));
            let arm = out.lanes[r].iter().any(|l| at_end[l.arm].iter().flatten().any(|&i| !opened[i]));
            seam || arm
        })
        .collect();
    let far: Vec<String> =
        moves.iter().filter(|m| m.0 > b.free_max_m).map(|m| format!("{} {:.0} m", place(m.1), m.0)).collect();
    let wall_m: f64 = walls.iter().map(|w| (w.1[1] - w.1[0]).abs()).sum();
    let ends = |e: End| found.iter().flat_map(|r| r.ends).filter(|&x| x == e).count();
    out.metrics = json!({
        "runs": found.len(),
        "runs_too_short": short,
        "run_ends": {"opens": ends(End::Opens), "seam": ends(End::Seam), "turns": ends(End::Turns)},
        "rooms": out.lanes.iter().filter(|l| l.len() > 1).count(),
        "seams": out.seams.len(),
        "caps": out.seams.iter().filter(|s| s.children.is_empty()).count(),
        "doors": out.seams.iter().map(|s| s.doors.len()).sum::<usize>(),
        "seams_not_whole": out.seams.iter().filter(|s| !s.whole).count(),
        "lanes_in_a_wall": walls.len(),
        "lanes_in_a_wall_m": (wall_m * 100.0).round() / 100.0,
        "lanes_in_a_wall_places": walls.iter().take(10).map(|w| format!("{} {:.1} m", place(arms[w.0].curve.at(w.1[0]).0.p), (w.1[1] - w.1[0]).abs())).collect::<Vec<_>>().join("; "),
        "cuts_without_one_room_on_a_side": no_parent.len(),
        "cuts_without_one_room_on_a_side_places": no_parent.iter().take(10).map(|&p| place(p)).collect::<Vec<_>>().join("; "),
        "lanes_off_the_line": off_line.len(),
        "lanes_off_the_line_places": off_line.join("; "),
        "lane_moves_over_free_max": far.len(),
        "lane_moves_over_free_max_places": far.join("; "),
        "farthest_lane_move_m": (moves.iter().map(|m| m.0).fold(0.0, f64::max) * 100.0).round() / 100.0,
    });
    // The rooms over others: of the upper arm and of the lower, those whose lanes run where the two lie over one
    // another.
    let lanes = &out.lanes;
    let of_arm = |arm: usize, [lo, hi]: [f64; 2]| {
        let meets =
            |l: &Lane| l.arm == arm && l.span[0].min(l.span[1]) < hi + 1e-6 && l.span[0].max(l.span[1]) > lo - 1e-6;
        (0..lanes.len()).filter(|&r| lanes[r].iter().any(meets)).collect::<Vec<usize>>()
    };
    let mut over: Vec<(usize, usize, bool)> = Vec::new();
    for &(upper, up, lower, low, height_only) in &over_arms {
        for u in of_arm(upper, up) {
            over.extend(of_arm(lower, low).into_iter().filter(|&t| t != u).map(|t| (u, t, height_only)));
        }
    }
    over.sort_unstable_by_key(|o| (o.0, o.1, o.2));
    over.dedup();
    out.over = over;
    out.arms = arms;
    out.arm_openings = moved_openings;
    (out, openings)
}
