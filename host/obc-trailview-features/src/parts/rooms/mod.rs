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
pub(crate) use section::{hold, onto, Outline};
pub(crate) use touch::{touching, Drawn};

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
    /// Per room end behind a seam, the metres from its hole within which it has no vertex (`section::clear`).
    pub clear: Vec<[Option<f64>; 2]>,
    /// Per room whether the rooms alone keep it from drawing: a seam at one of its ends holds no hole for a room behind
    /// it, or an arm of it has an opening in no room.
    pub broken: Vec<bool>,
    /// Per room whether it is the whole of one arm, with no seam: a tunnel of one way.
    pub alone: Vec<bool>,
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

    /// Per room whether it can draw as parts on its own account: it is not `out` nor `broken`, and each of its ends is a
    /// seam or of a built kind (`kind` of its opening), as the builder builds a chord.
    pub fn own(&self, kind: &dyn Fn(usize, usize) -> Option<Kind>, out: &[bool]) -> Vec<bool> {
        let end = |t: usize, e: usize| self.seam_at(t, e) || obc_trailview_parts::built(&[kind(t, e)]);
        (0..self.tubes.len()).map(|t| !out[t] && !self.broken[t] && (0..2).all(|e| end(t, e))).collect()
    }

    /// Per room whether it draws as parts: where it can on its own account (`own`), and so can each room of its seams
    /// and of its arms. The rooms of one seam and the rooms of one arm draw together or not at all.
    pub fn built(&self, kind: &dyn Fn(usize, usize) -> Option<Kind>, out: &[bool]) -> Vec<bool> {
        let n = self.tubes.len();
        let own = self.own(kind, out);
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
        for t in (0..n).filter(|&t| !own[t]) {
            let r = find(&mut root, t);
            ok[r] = false;
        }
        (0..n).map(|t| ok[find(&mut root, t)]).collect()
    }

    /// Per room whether it has a box section: where it has an underpass at an end (`boxed`), or another room of a seam
    /// of it has; the rooms of one seam have one section, so each hole fits the room before the line.
    pub fn one_section(&self, mut boxed: Vec<bool>) -> Vec<bool> {
        loop {
            let mut changed = false;
            for seam in &self.seams {
                let members: Vec<usize> = [seam.parent].iter().chain(&seam.children).map(|m| m.0).collect();
                if members.iter().any(|&r| boxed[r]) {
                    for r in members {
                        changed |= !boxed[r];
                        boxed[r] = true;
                    }
                }
            }
            if !changed {
                return boxed;
            }
        }
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
/// section per vertex, its openings, its two ends, the `clear` of each end behind a seam, and whether its cap has no
/// metres to lie a wall from the room before its door.
#[derive(Clone)]
struct Made {
    tube: Tube,
    from: f64,
    lanes: Vec<Lane>,
    table: Vec<(f64, Vec<Place>)>,
    openings: Vec<Opening>,
    bounds: [Bound; 2],
    clear: [Option<f64>; 2],
    cramped: bool,
}

/// The rooms of the arms (`cuts`), from the runs of two arms (`runs`). A run ends where an arm opens, at the openings of
/// both arms on that side; else in the hill at its last metre, a seam. Where the next metres are across, of its two
/// arms the narrower (then the higher id) turns away: its lane ends on the seam's line with a door, and its arm is in
/// no room until the first metre `vertex_gap_m` or more past it where it is with or across no arm; there its room starts
/// with a cap. A seam's holes come from the row of the room before it (`holes`). The lanes of a room open on one frame
/// line at each end with openings (`section`).
/// The rooms of the arms; an arm that is `lone` runs beside no other.
pub(crate) fn rooms(
    arms: Vec<Tube>,
    arm_openings: Vec<Opening>,
    lone: &[bool],
    input: &Input,
) -> (Rooms, Vec<Opening>) {
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
    // An arm folds where its axis comes within its width and a wall of itself, at metres more than twice that apart
    // along it, and does not lie clear over itself there (a V or a hairpin; a spiral passes over itself): its walls
    // would stand in its own room.
    let axes = Axes::new(&arms);
    let wall = 2.0 * b.shell_m;
    let folds = |t: usize| {
        let (a, reach) = (&arms[t], 2.0 * arms[t].half + wall);
        let over = a.height + b.shell_m + b.slab_m;
        let steps = a.curve.len.ceil().max(1.0) as usize;
        let at = |k: usize| a.curve.len * k as f64 / steps as f64;
        (0..=steps).map(at).find(|&s| {
            let q = a.curve.at(s).0;
            axes.folds(t, (q.p, s, q.z), (reach, 2.0 * reach, over))
        })
    };
    let folded: Vec<Option<[f64; 2]>> = (0..n).map(|t| folds(t).map(|s| arms[t].curve.at(s).0.p)).collect();
    // An arm runs beside others where it can draw: it opens at both ends, does not fold, its heights are known, it has a
    // length and it is not lone.
    let live: Vec<Option<[f64; 2]>> = (0..n)
        .map(|t| {
            let can = at_end[t].iter().all(Option::is_some) && folded[t].is_none() && arms[t].heights && !lone[t];
            spans[t].filter(|s| can && s[1] - s[0] > 1e-6)
        })
        .collect();
    let metres = runs::metres(&arms, &live, input);
    let (found, over_arms, short) = runs::runs(&metres, &live, input);
    let turned = runs::turned(&found, &metres, (&arms, &live), input);
    let cuts::Cut { mut rooms, seams, mut walls, no_parent } =
        cuts::cut(&arms, (&spans, &at_end), &found, &turned, input);
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
        clear: Vec::new(),
        broken: Vec::new(),
        alone: Vec::new(),
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
    // The metres along arm `arm` of its door at seam sd: where its lane in the room before the line ends there, and it
    // is in no room behind it.
    let door_at = |sd: usize, arm: usize| {
        let seam = &out.seams[sd];
        let behind = seam.children.iter().any(|c| rooms[c.0].0.iter().any(|l| l.arm == arm));
        let lane =
            rooms[seam.parent.0].0.iter().find(|l| l.arm == arm).filter(|_| !seam.children.is_empty() && !behind);
        lane.map(|l| l.span[seam.parent.1])
    };
    // Per room of one lane, each end at a cap, with the cap's seam, and the seam of the door on its arm nearest it and
    // the door's metres.
    let caps: Vec<Vec<(usize, usize, usize, f64)>> = (0..rooms.len())
        .map(|r| {
            let [l] = &rooms[r].0[..] else { return Vec::new() };
            let cap = |e: usize| {
                let Bound::Seam(sc) = rooms[r].1[e] else { return None };
                let doors = (0..out.seams.len()).filter_map(|sd| Some((sd, door_at(sd, l.arm)?)));
                let near = doors.min_by(|x, y| (x.1 - l.span[e]).abs().total_cmp(&(y.1 - l.span[e]).abs()));
                near.filter(|_| out.seams[sc].children.is_empty()).map(|(sd, from)| (e, sc, sd, from))
            };
            (0..2).filter_map(cap).collect()
        })
        .collect();
    // Each room after the room before each seam at which it lies behind the line, and a room with a cap after the room
    // before its door: a seam's holes come from the row of the room before it, a room behind the line starts at its
    // hole, and a cap lies a wall from the room before its door. A room of one lane is its arm, or the stretch of its
    // arm from its metres there on.
    let parents = |r: usize| {
        let seams = out.seams.iter().filter(move |s| s.children.iter().any(|c| c.0 == r));
        seams.map(|s| s.parent.0).chain(caps[r].iter().map(|c| out.seams[c.2].parent.0)).collect::<Vec<usize>>()
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
    let clear_of = |sideways: f64, half: f64| section::clear(sideways, half, b);
    // A room's half-width as its records hold it: up to the centimetre for a room of lanes.
    let records_half = |m: &Made| {
        let half = if m.lanes.len() < 2 { m.tube.half } else { super::ceil_cm(m.tube.half) };
        (half * 100.0).round() / 100.0
    };
    let mitre = obc_trailview_parts::Rules::embedded().design.mitre_max;
    let mut starts: BTreeMap<(usize, usize), Start> = BTreeMap::new();
    let mut made: Vec<Option<Made>> = vec![None; rooms.len()];
    for &r in &order {
        let (lanes, mut bounds) = rooms[r].clone();
        let start = [0, 1].map(|e| starts.get(&(r, e)));
        let room = if lanes.len() > 1 {
            let (tube, ordered, table, ends, moved, off, clear) =
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
            Made { tube, from: 0.0, lanes: ordered, table, openings: opens, bounds, clear, cramped: false }
        } else {
            let mut l = lanes[0].clone();
            let t = &arms[l.arm];
            let [s0, s1] = spans[l.arm].unwrap();
            // A cap moves on along its arm, a metre at a time, until its face lies a wall from the room before its door
            // as the builder draws that room's chord at the door's seam; a room with no metres for that cannot draw.
            let mut cramped = false;
            for &(e, sc, sd, from) in &caps[r] {
                let door = out.seams[sd].clone();
                let Some(before) = made[door.parent.0].as_ref() else { continue };
                let Some(end) = touch::end_chord(&before.tube, door.parent.1, (door.dir, records_half(before)), mitre)
                else {
                    continue;
                };
                let face = |s: f64| {
                    let (p, left) = (t.curve.at(s).0.p, left_of(t.curve.dir(s)));
                    [offset(p, left, -t.half), offset(p, left, t.half)]
                };
                let (away, step) = ((l.span[e] - from).signum(), (s1 - s0) / (s1 - s0).ceil().max(1.0));
                let mut at = l.span[e];
                while !cramped && touch::from_end(&end, face(at)) < 2.0 * b.shell_m - MERGE_M {
                    at += away * step;
                    cramped = away * (l.span[1 - e] - at) < b.vertex_gap_m;
                }
                if cramped {
                    continue;
                }
                if let Some(w) =
                    walls.iter_mut().find(|w| w.0 == l.arm && w.1.iter().any(|&x| (x - l.span[e]).abs() < 1e-6))
                {
                    let k = usize::from((w.1[1] - l.span[e]).abs() < 1e-6);
                    w.1[k] = at;
                }
                l.span[e] = at;
                let (p, d, toward) = (t.curve.at(at).0.p, t.curve.dir(at), t.curve.at(l.span[1 - e]).0.p);
                out.seams[sc].p = p;
                out.seams[sc].dir = if along_dir(p, toward, d) < 0.0 { d } else { [-d[0], -d[1]] };
            }
            let l = &l;
            let whole = (l.span[0] - s0).abs() < 1e-6 && (l.span[1] - s1).abs() < 1e-6;
            let (mut tube, from) = if whole { (t.clone(), 0.0) } else { (stretch(t, l.span), l.span[0]) };
            let mut clear = [None; 2];
            for (e, st) in start.iter().enumerate() {
                if let Some(st) = st {
                    let own = tube.curve.at(if e == 0 { 0.0 } else { tube.curve.len }).0.p;
                    let c = clear_of(distance(st.p, own), tube.half);
                    started(&mut tube, e, st.p, c);
                    clear[e] = Some(c);
                }
            }
            let opens = (l.ends.iter().flatten()).map(|&i| Opening {
                tube: r,
                s: arm_openings[i].s - from,
                ..arm_openings[i].clone()
            });
            let openings = opens.collect();
            Made { tube, from, lanes: vec![l.clone()], table: Vec::new(), openings, bounds, clear, cramped }
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
            match holes_of(&row, (&children, records_half(&room)), b.shell_m).filter(|_| !children.is_empty()) {
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
    let mut cramped = Vec::new();
    for room in made.into_iter().map(|m| m.expect("each room is made")) {
        cramped.push(room.cramped);
        openings.extend(room.openings);
        out.tubes.push(room.tube);
        out.from.push(room.from);
        out.lanes.push(room.lanes);
        out.table.push(room.table);
        out.bounds.push(room.bounds);
        out.clear.push(room.clear);
    }
    // A room behind a seam made before the room in front of it has no hole.
    for s in 0..out.seams.len() {
        out.seams[s].whole &= out.seams[s].holes.len() == out.seams[s].children.len();
    }
    // A room cannot draw where a seam at its ends is not whole, an arm of it has an opening in no room, its cap has no
    // metres to lie a wall from the room before its door, or it lies behind a seam and is shorter than the clear of its
    // two ends (`vertex_gap_m` at an end with none): it holds no vertex outside them.
    let mut opened = vec![false; arm_openings.len()];
    for l in out.lanes.iter().flatten() {
        l.ends.iter().flatten().for_each(|&i| opened[i] = true);
    }
    out.broken = (0..out.tubes.len())
        .map(|r| {
            let seam = (0..2).any(|e| matches!(out.bounds[r][e], Bound::Seam(s) if !out.seams[s].whole));
            let arm = out.lanes[r].iter().any(|l| at_end[l.arm].iter().flatten().any(|&i| !opened[i]));
            let [c0, c1] = out.clear[r].map(|c| c.unwrap_or(b.vertex_gap_m));
            let short = out.clear[r].iter().any(Option::is_some) && out.tubes[r].curve.len < c0 + c1;
            seam || arm || cramped[r] || short
        })
        .collect();
    out.alone = (0..out.tubes.len())
        .map(|r| {
            let [l] = &out.lanes[r][..] else { return false };
            let whole = out.from[r] == 0.0 && (out.tubes[r].curve.len - arms[l.arm].curve.len).abs() < 1e-6;
            whole && (0..2).all(|e| !out.seam_at(r, e))
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
        "arms_that_fold": folded.iter().flatten().count(),
        "arms_that_fold_places": folded.iter().flatten().take(10).map(|&p| place(p)).collect::<Vec<_>>().join("; "),
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
    // Each two rooms once; where one arm reads them as crossing, they cross.
    over.sort_unstable_by_key(|o| (o.0, o.1, o.2));
    over.dedup_by_key(|o| (o.0, o.1));
    out.over = over;
    out.arms = arms;
    out.arm_openings = moved_openings;
    (out, openings)
}
