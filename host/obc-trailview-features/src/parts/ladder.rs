//! The ladder of the parts: per opening every kind of part that fits, in the order of the rungs, and the numbers of
//! the first.

use std::collections::HashMap;

use serde_json::{json, Value};

use obc_ground_plan::graph::OpenEnd;

use super::floor::{self, Floor};
use super::measure::{self, Ask, Site};
use super::{along_dir, counts, distance, first_way_m, left_of, open_ends, Input, Opening, Tube};

/// A kind of part at an opening.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Pair,
    Rock,
    Arch,
    Box,
    Free,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Pair => "pair",
            Kind::Rock => "rock",
            Kind::Arch => "arch",
            Kind::Box => "box",
            Kind::Free => "free",
        }
    }
}

/// The numbers of a drawn opening (Plan 8 of the design): metres, `left` and `right` per metre out from the frame's
/// outer edge, and in a pair the mate's opening and the metres to its axis to the left; `height` is the inside height
/// of the tube's chord there.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Numbers {
    pub height: f64,
    pub collar: f64,
    pub face: f64,
    pub left: Vec<f64>,
    pub right: Vec<f64>,
    pub foot: f64,
    pub mate: Option<(usize, f64)>,
}

/// An opening's decision: every kind that fits in the order of the rungs, the part of the end (the first; also the
/// mate of a pair), the rung that decided it, and the numbers where the opening has a record of its own.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct End {
    pub kinds: Vec<Kind>,
    pub kind: Option<Kind>,
    pub rung: &'static str,
    pub numbers: Option<Numbers>,
}

/// The decisions of all openings, the inside half-width and the floor of each tube (None with no part), and the
/// counts.
pub(crate) struct Decided {
    pub ends: Vec<End>,
    pub half: Vec<f64>,
    pub floors: Vec<Option<Floor>>,
    pub metrics: Value,
}

/// The measure of the asks, with the open ends of all openings.
pub(crate) type Measure<'a> = dyn Fn(&[Ask], &[OpenEnd]) -> (Vec<Site>, usize) + 'a;

/// Two openings of other tubes that lead the same way within `pair_max_deg` and lie within `pair_max_m`: their
/// metres apart, and b's metres to the left of a and in along a.
struct Near {
    a: usize,
    b: usize,
    d: f64,
    lat: f64,
    stag: f64,
}

/// Every pair of openings of different tubes within `pair_max_m` and `pair_max_deg`, nearest first.
fn near(openings: &[Opening], input: &Input) -> Vec<Near> {
    let b = &input.rules.bake;
    let cos = b.pair_max_deg.to_radians().cos();
    // One grid for all openings, its cells at least `pair_max_m` wide at each of them.
    let size = openings.iter().map(|o| b.pair_max_m.max(1.0) / crate::metres(o.p[1])).fold(0.0, f64::max);
    let cell = |p: [f64; 2]| ((p[0] / size).floor() as i64, (p[1] / size).floor() as i64);
    let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, o) in openings.iter().enumerate() {
        grid.entry(cell(o.p)).or_default().push(i);
    }
    let mut out = Vec::new();
    for (i, o) in openings.iter().enumerate() {
        let (cx, cy) = cell(o.p);
        for j in (-1..=1).flat_map(|dx| (-1..=1).map(move |dy| (cx + dx, cy + dy))).filter_map(|c| grid.get(&c)) {
            for &k in j.iter().filter(|&&k| k > i) {
                let q = &openings[k];
                if q.tube == o.tube || o.dir[0] * q.dir[0] + o.dir[1] * q.dir[1] < cos {
                    continue;
                }
                let d = distance(o.p, q.p);
                if d <= b.pair_max_m {
                    let (lat, stag) = (along_dir(o.p, q.p, left_of(o.dir)), along_dir(o.p, q.p, o.dir));
                    out.push(Near { a: i, b: k, d, lat, stag });
                }
            }
        }
    }
    out.sort_by(|x, y| x.d.total_cmp(&y.d).then((x.a, x.b).cmp(&(y.a, y.b))));
    out
}

/// An opening is measured again where a changed open stretch lies this near: its reads reach about 100 m (the stations
/// in, up to `free_max_m` and the longest stagger of a pair, and the row across, up to `pair_max_m` and two tubes with
/// their wings), and a stretch changes the ground up to about 100 m from its end vertex (its length, at most
/// `junction_shift_max_m` plus `pair_max_m`, and the cut and fill of its road beyond it).
const AGAIN_M: f64 = 200.0;
const SQUARE: f64 = (1u64 << obc_ground_plan::frame::FRAME_BITS) as f64;

/// What to read about opening i (`measure::ask`): as far across and in as a pair with a candidate can need (`reach`:
/// metres to the right, to the left, and the longest stagger).
fn ask_of(
    i: usize,
    openings: &[Opening],
    tubes: &[Tube],
    half: &[f64],
    reach: &[(f64, f64, f64)],
    input: &Input,
) -> Ask {
    let b = &input.rules.bake;
    let o = &openings[i];
    let t = &tubes[o.tube];
    let g = &input.rules.groups[t.group];
    let in_m = g.collar_max_m.max(b.free_max_m) + reach[i].2;
    let sign = if o.end == 0 { 1.0 } else { -1.0 };
    let (right, left) = (reach[i].0.ceil() as u32, reach[i].1.ceil() as u32);
    measure::ask(&t.curve, (o.s, sign), (o.p, o.dir), (right, left), in_m, half[o.tube] + b.shell_m, t.ids.clone())
}

/// The first metre in from the opening (station index) whose cover reaches `need`: the least ground over the centre
/// and both walls, less the floor there.
fn first_covered(site: &Site, floors: &[f64], need: f64) -> Option<usize> {
    (site.stations.iter().zip(floors)).position(|(st, &f)| {
        let g: Option<Vec<f64>> = st.iter().copied().collect();
        g.is_some_and(|g| g.iter().fold(f64::INFINITY, |a, &b| a.min(b)) - f >= need)
    })
}

/// The kinds, numbers and counts of the openings; a pair moves its nearer opening in onto the frame line of the other.
pub(crate) fn decide(tubes: &[Tube], openings: &mut [Opening], input: &Input, measure: &Measure) -> Decided {
    let b = &input.rules.bake;
    let n = openings.len();
    let mut rung: Vec<Option<&'static str>> = vec![None; n];
    let mut half: Vec<f64> = tubes.iter().map(|t| t.half).collect();
    let height: Vec<f64> = tubes.iter().map(|t| t.height).collect();
    // The two openings of a chain shorter than their shifts would meet: they stay at the ends of the chain.
    let mut met = 0;
    for i in 0..n {
        let Some(k) = (i + 1..n).find(|&k| openings[k].tube == openings[i].tube) else { continue };
        let (a, c) = if openings[i].end == 0 { (i, k) } else { (k, i) };
        if openings[a].s >= openings[c].s {
            let t = &tubes[openings[i].tube];
            for j in [a, c] {
                openings[j] = openings[j].at(t, 0.0);
            }
            met += 1;
        }
    }
    for (i, o) in openings.iter().enumerate() {
        if !tubes[o.tube].heights {
            rung[i] = Some("a vertex of unknown height");
        }
    }
    let near = near(openings, input);
    // Rung 2: an end in the tube of a wider one; the widest takes first.
    let mut same: Vec<(usize, usize, f64, f64)> = Vec::new();
    for x in &near {
        let rank = |i: usize| (-tubes[openings[i].tube].half, openings[i].way_id, i);
        let (w, s) = if rank(x.a) <= rank(x.b) { (x.a, x.b) } else { (x.b, x.a) };
        let (ow, os) = (&openings[w], &openings[s]);
        let (lat, stag) = (along_dir(ow.p, os.p, left_of(ow.dir)), along_dir(ow.p, os.p, ow.dir));
        if lat.abs() <= tubes[ow.tube].half + b.same_tube_gap_m && stag.abs() <= b.same_tube_stagger_m {
            same.push((w, s, lat, x.d));
        }
    }
    same.sort_by(|x, y| {
        let h = |i: usize| tubes[openings[i].tube].half;
        h(y.0).total_cmp(&h(x.0)).then(x.3.total_cmp(&y.3)).then((x.0, x.1).cmp(&(y.0, y.1)))
    });
    for &(w, s, lat, _) in &same {
        if rung[w].is_some() || rung[s].is_some() {
            continue;
        }
        rung[s] = Some("same tube");
        let (tw, ts) = (openings[w].tube, openings[s].tube);
        half[tw] = half[tw].max(lat.abs() + tubes[ts].way_half + tubes[tw].margin);
    }
    // The measure, as far across and in as a pair with a candidate can need.
    let free: Vec<bool> = rung.iter().map(Option::is_none).collect();
    let mut reach: Vec<(f64, f64, f64)> = (0..n)
        .map(|i| {
            let t = &tubes[openings[i].tube];
            let side = half[openings[i].tube] + b.shell_m + input.rules.groups[t.group].wing_max_m;
            (side, side, 0.0)
        })
        .collect();
    for x in near
        .iter()
        .filter(|x| free[x.a] && free[x.b] && tubes[openings[x.a].tube].rail == tubes[openings[x.b].tube].rail)
    {
        for (i, k, lat, stag) in [(x.a, x.b, x.lat, x.stag), (x.b, x.a, -x.lat, -x.stag)] {
            let tk = &tubes[openings[k].tube];
            let far = lat.abs() + half[openings[k].tube] + b.shell_m + input.rules.groups[tk.group].wing_max_m;
            let r = &mut reach[i];
            if lat >= 0.0 {
                r.1 = r.1.max(far);
            } else {
                r.0 = r.0.max(far);
            }
            r.2 = r.2.max(stag.abs());
        }
    }
    let asked: Vec<usize> = (0..n).filter(|&i| free[i]).collect();
    let floors_of = |o: &Opening, len: usize| -> Vec<f64> {
        let t = &tubes[o.tube];
        let sign = if o.end == 0 { 1.0 } else { -1.0 };
        (0..len).map(|k| t.curve.at(o.s + sign * k as f64).0.z).collect()
    };
    let asks: Vec<Ask> = asked.iter().map(|&i| ask_of(i, openings, tubes, &half, &reach, input)).collect();
    let first = open_ends(openings);
    let (measured, mut cells) = measure(&asks, &first);
    let mut sites: Vec<Option<Site>> = vec![None; n];
    let mut rights: Vec<u32> = vec![0; n];
    for ((&i, site), a) in asked.iter().zip(measured).zip(&asks) {
        sites[i] = Some(site);
        rights[i] = a.right;
    }
    let need = |t: usize| tubes[t].height + b.shell_m + b.cover_m;
    let mut cover: Vec<Option<f64>> = (0..n)
        .map(|i| {
            let site = sites[i].as_ref()?;
            let o = &openings[i];
            first_covered(site, &floors_of(o, site.stations.len()), need(o.tube)).map(|k| k as f64)
        })
        .collect();
    // Rung 3 reads the rooms and the buildings of both ends of a tube.
    let mut rooms: Vec<Vec<f64>> = vec![Vec::new(); tubes.len()];
    let mut building = vec![false; tubes.len()];
    for (i, site) in sites.iter().enumerate() {
        if let Some(site) = site {
            rooms[openings[i].tube].extend(&site.rooms);
            building[openings[i].tube] |= site.building;
        }
    }
    let mut kinds: Vec<Vec<Kind>> = vec![Vec::new(); n];
    let mut hill: Vec<usize> = Vec::new();
    for i in (0..n).filter(|&i| free[i]) {
        let o = &openings[i];
        let t = &tubes[o.tube];
        let g = &input.rules.groups[t.group];
        let short = t.curve.len < b.underpass_max_m;
        let d = cover[i];
        let (r, k): (&'static str, Vec<Kind>) = if short && !rooms[o.tube].is_empty() {
            ("underpass", vec![Kind::Box, Kind::Free])
        } else if short && building[o.tube] {
            ("under a building", Vec::new())
        } else if o.others == 0 && d == Some(0.0) {
            ("free: a covered end that meets no way", vec![Kind::Free])
        } else if d.is_some_and(|d| d <= g.collar_max_m) {
            hill.push(i);
            let rock = g.rock && d.is_some_and(|d| d <= b.rock_cover_max_m);
            ("hill", if rock { vec![Kind::Rock, Kind::Arch, Kind::Free] } else { vec![Kind::Arch, Kind::Free] })
        } else if d.is_none() {
            ("free: never covered", vec![Kind::Free])
        } else {
            ("free: covered past the limit", vec![Kind::Free])
        };
        rung[i] = Some(r);
        kinds[i] = k;
    }
    // Rung 5: the pairs, nearest first, each end in one pair at most.
    let mut mate: Vec<Option<usize>> = vec![None; n];
    let is_hill = |i: usize| hill.contains(&i);
    for x in &near {
        let (a, c) = (x.a, x.b);
        if !is_hill(a) || !is_hill(c) || mate[a].is_some() || mate[c].is_some() {
            continue;
        }
        if tubes[openings[a].tube].rail != tubes[openings[c].tube].rail {
            continue;
        }
        mate[a] = Some(c);
        mate[c] = Some(a);
        kinds[a].insert(0, Kind::Pair);
        kinds[c].insert(0, Kind::Pair);
    }
    // A pair's frame line is the line of the opening farther in: the nearer one moves in onto it along its way.
    let mut moved: Vec<f64> = vec![0.0; n];
    let mut far_of: Vec<usize> = (0..n).collect();
    let mut unpaired = 0;
    for i in 0..n {
        let Some(m) = mate[i].filter(|&m| m > i) else { continue };
        let (far, nearer) =
            if along_dir(openings[i].p, openings[m].p, openings[i].dir) > 0.0 { (m, i) } else { (i, m) };
        let (pf, df) = (openings[far].p, openings[far].dir);
        let o = openings[nearer].clone();
        let t = &tubes[o.tube];
        let most = (o.shift + b.pair_max_m).min(t.curve.len / 2.0).min(first_way_m(t, o.end));
        let off = |shift: f64| along_dir(pf, o.at(t, shift).p, df);
        let (mut lo, mut hi) = (o.shift, most.max(o.shift));
        // Where its way does not reach the line within its limits, the two are no pair: each takes its next kind.
        if off(hi) < 0.0 {
            for k in [i, m] {
                mate[k] = None;
                kinds[k].retain(|&x| x != Kind::Pair);
            }
            unpaired += 1;
            continue;
        }
        for _ in 0..40 {
            let mid = (lo + hi) / 2.0;
            if off(mid) < 0.0 {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        moved[nearer] = hi - o.shift;
        openings[nearer] = Opening { others: o.others, ..o.at(t, hi) };
        far_of[i] = far;
        far_of[m] = far;
    }
    // The numbers come from the ground with the stretches that the records open: of each opening with a part, at its
    // final place. The openings near a stretch that the first measure opened otherwise are measured again there.
    let with_part: Vec<Opening> = (0..n).filter(|&i| !kinds[i].is_empty()).map(|i| openings[i].clone()).collect();
    let last = open_ends(&with_part);
    let changed: Vec<[f64; 2]> = (first.iter().filter(|e| !last.contains(e)))
        .chain(last.iter().filter(|e| !first.contains(e)))
        .map(|e| [e.way.x as f64 / SQUARE, e.way.y as f64 / SQUARE])
        .collect();
    let again: Vec<usize> = (0..n)
        .filter(|&i| !kinds[i].is_empty() && changed.iter().any(|&p| distance(p, openings[i].p) <= AGAIN_M))
        .collect();
    if !again.is_empty() {
        let asks: Vec<Ask> = again.iter().map(|&i| ask_of(i, openings, tubes, &half, &reach, input)).collect();
        let (measured, more) = measure(&asks, &last);
        cells += more;
        for ((&i, site), a) in again.iter().zip(measured).zip(&asks) {
            let o = &openings[i];
            cover[i] = first_covered(&site, &floors_of(o, site.stations.len()), need(o.tube)).map(|k| k as f64);
            (sites[i], rights[i], moved[i]) = (Some(site), a.right, 0.0);
        }
    }
    // Where the ground of the second measure no longer fits a kind (no cover, or cover past the limit), the end drops
    // it; a pair with an end that dropped `Pair` is no pair. Dropping kinds commutes, so the order of the ends does not
    // matter.
    let before: Vec<Option<Kind>> = kinds.iter().map(|k| k.first().copied()).collect();
    for &i in &again {
        let g = &input.rules.groups[tubes[openings[i].tube].group];
        let d = cover[i];
        let fits = |k: &Kind| match k {
            Kind::Pair | Kind::Arch => d.is_some_and(|d| d <= g.collar_max_m),
            Kind::Rock => d.is_some_and(|d| d <= g.collar_max_m.min(b.rock_cover_max_m)),
            Kind::Box | Kind::Free => true,
        };
        kinds[i].retain(|k| fits(k));
        if before[i] != Some(Kind::Free) && kinds[i].first() == Some(&Kind::Free) {
            rung[i] = Some(if d.is_none() { "free: never covered" } else { "free: covered past the limit" });
        }
        if let Some(m) = mate[i].filter(|_| !kinds[i].contains(&Kind::Pair)) {
            (mate[i], mate[m]) = (None, None);
            kinds[m].retain(|&k| k != Kind::Pair);
            (far_of[i], far_of[m]) = (i, m);
        }
    }
    let refitted = (0..n).filter(|&i| kinds[i].first().copied() != before[i]).count();
    // The floor of each tube with a part, from opening to opening; an end with no opening holds the end vertex. Its
    // chord at an opening gives the opening its inside height.
    let spans: Vec<Option<[f64; 2]>> = (0..tubes.len())
        .map(|t| {
            let at = |end: usize| openings.iter().position(|o| o.tube == t && o.end == end);
            let with = [at(0), at(1)].into_iter().flatten().any(|i| !kinds[i].is_empty());
            let s0 = at(0).map_or(0.0, |i| openings[i].s);
            let s1 = at(1).map_or(tubes[t].curve.len, |i| openings[i].s);
            (with && s1 - s0 > 1e-6).then_some([s0, s1])
        })
        .collect();
    let (floors, floor_metrics) = floor::floors(tubes, &spans, (&half, &height), input);
    let end_height = |o: &Opening| floors[o.tube].as_ref().map_or(height[o.tube], |f| f.end_height(o.end));
    // The numbers of each drawn opening.
    let mut ends: Vec<End> = Vec::with_capacity(n);
    for i in 0..n {
        let kind = kinds[i].first().copied();
        // A pair has one record, at the opening of the lower feature id.
        let drawn = mate[i].is_none_or(|m| (openings[i].way_id, i) < (openings[m].way_id, m));
        let m = Measured { sites: &sites, rights: &rights, cover: &cover, moved: &moved };
        let h = end_height(&openings[i]);
        let numbers = (kind.filter(|_| drawn))
            .map(|kind| numbers(i, kind, (mate[i], far_of[i]), &m, (openings, tubes), (&half, h), input));
        ends.push(End { kinds: kinds[i].clone(), kind, rung: rung[i].unwrap_or_default(), numbers });
    }
    let shown = |e: &End| match e.kind {
        Some(k) => k.name().to_string(),
        None => format!("none: {}", e.rung),
    };
    let mut metrics = json!({
        "tubes": tubes.len(),
        "openings": n,
        "rungs": counts(ends.iter().map(|e| e.rung.to_string())),
        "ends": counts(ends.iter().map(shown)),
        "end_records": ends.iter().filter(|e| e.numbers.is_some()).count(),
        "cells": cells,
        "measured_again": again.len(),
        "pairs_that_do_not_reach": unpaired,
        "kinds_changed_by_the_second_measure": refitted,
        "chains_whose_openings_would_meet": met,
    });
    if let (Value::Object(m), Value::Object(f)) = (&mut metrics, floor_metrics) {
        m.extend(f);
    }
    Decided { ends, half, floors, metrics }
}

/// What the measure gave of each opening: its site, the metres of its row to the right, the metres to its first covered
/// metre, and the metres that a pair moved it in.
#[derive(Clone, Copy)]
struct Measured<'a> {
    sites: &'a [Option<Site>],
    rights: &'a [u32],
    cover: &'a [Option<f64>],
    moved: &'a [f64],
}

/// The numbers of drawn opening i of `kind` (Plan 8) with the inside height `h` there: its frame is its own, or in a
/// pair the frame line of the opening farther in, which holds both.
fn numbers(
    i: usize,
    kind: Kind,
    (mate, far): (Option<usize>, usize),
    m: &Measured,
    (openings, tubes): (&[Opening], &[Tube]),
    (half, h): (&[f64], f64),
    input: &Input,
) -> Numbers {
    let Measured { sites, rights, cover, moved } = *m;
    let b = &input.rules.bake;
    let o = &openings[i];
    let t = &tubes[o.tube];
    let g = &input.rules.groups[t.group];
    let floor = o.floor;
    let site = sites[far].as_ref().expect("a measured opening");
    let left_far = left_of(openings[far].dir);
    let axis = |k: usize| along_dir(openings[far].p, openings[k].p, left_far);
    let edge = |k: usize| half[openings[k].tube] + b.shell_m;
    let mut members = vec![i];
    members.extend(mate);
    let lo = members.iter().map(|&k| axis(k) - edge(k)).fold(f64::INFINITY, f64::min);
    let hi = members.iter().map(|&k| axis(k) + edge(k)).fold(f64::NEG_INFINITY, f64::max);
    let ground = |x: f64| site.across(rights[far], x);
    let frame: Vec<f64> = std::iter::once(lo)
        .chain((lo.ceil() as i64..=hi.floor() as i64).map(|x| x as f64))
        .chain([hi])
        .filter_map(ground)
        .collect();
    let highest = frame.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let mut face = (h + b.shell_m + b.band_m).max(highest - floor + b.coping_m);
    face = face.min(h + b.shell_m + b.face_over_crown_max_m);
    // A deck over the frame line or its wing walls: its underside lies over the floor (a way that goes on from the
    // opening on a deck lies at the floor, not over it).
    for &(x, under) in &site.decks {
        if x >= lo - g.wing_max_m && x <= hi + g.wing_max_m && under - b.coping_m > floor {
            face = face.min(under - b.coping_m - floor);
        }
    }
    let wing = |from: f64, sign: f64| {
        let mut row = Vec::new();
        let mut d = 1.0;
        while d <= g.wing_max_m {
            let gr = ground(from + sign * d);
            // A wall's top lies at least its coping over the floor.
            let top = |g: f64| (g + b.coping_m - floor).clamp(b.coping_m, face.max(b.coping_m));
            if gr.is_none_or(|v| v < floor + b.wing_ground_m) {
                if row.is_empty() {
                    row.push(top(gr.unwrap_or(floor)));
                }
                break;
            }
            row.push(top(gr.unwrap()));
            d += 1.0;
        }
        if row.is_empty() {
            row.push(b.coping_m);
        }
        row
    };
    let (left, right) = (wing(hi, 1.0), wing(lo, -1.0));
    let collar_of = |k: usize| cover[k].map(|d| (d - moved[k]).max(0.0));
    let half_chain = t.curve.len / 2.0;
    let collar = match kind {
        Kind::Pair => members.iter().filter_map(|&k| collar_of(k)).fold(0.0, f64::max),
        Kind::Arch | Kind::Rock => collar_of(i).expect("an arch is covered within its collar"),
        Kind::Box => collar_of(i).unwrap_or(f64::INFINITY).min(half_chain),
        Kind::Free => collar_of(i).unwrap_or(f64::INFINITY).min(b.free_max_m).min(half_chain),
    };
    // The lowest ground under the frame, its wing walls and the collar.
    let own = sites[i].as_ref().expect("a measured opening");
    let (from, to) = (lo - right.len() as f64, hi + left.len() as f64);
    let under = (from.ceil() as i64..=to.floor() as i64).filter_map(|x| ground(x as f64));
    let collar_ground = (own.stations.iter())
        .skip(moved[i].round() as usize)
        .take(collar.ceil() as usize + 1)
        .flat_map(|st| st.iter().flatten().copied());
    let lowest = under.chain(collar_ground).fold(floor, f64::min);
    let mate = mate.map(|m| (m, axis(m) - axis(i)));
    let numbers = if kind == Kind::Free { (Vec::new(), Vec::new()) } else { (left, right) };
    Numbers { height: h, collar, face, left: numbers.0, right: numbers.1, foot: lowest - b.foot_m - floor, mate }
}
