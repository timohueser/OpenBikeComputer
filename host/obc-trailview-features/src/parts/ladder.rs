//! The ladder of the parts: per opening every kind of part that fits, in the order of the rungs, and the numbers of
//! the first.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::{json, Value};

use obc_ground_plan::graph::OpenEnd;
use obc_trailview_parts::PAST_M;

use super::floor::{self, Floor};
use super::measure::{self, Ask, Site};
use super::rooms::Rooms;
use super::{along_dir, built_openings, counts, distance, first_way_m, left_of, open_ends, Input, Opening, Tube};

/// The kind of part at an opening.
pub use obc_trailview_parts::Part as Kind;

/// The numbers of a drawn head (Plan 8 of the design): metres over the floor of its first hole, `left` and `right` per
/// metre out from the frame's outer edge; per hole from the right its opening, the metres to its axis to the left, the
/// inside height of the tube's chord there and its collar; `heading` is square to its frame line, into the tube of its
/// group's far opening; `walls` whether the record holds `face`, `left` and `right`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Numbers {
    pub heading: f64,
    pub holes: Vec<Hole>,
    pub walls: bool,
    pub face: f64,
    pub left: Vec<f64>,
    pub right: Vec<f64>,
    pub foot: f64,
}

/// A hole of a head: its opening, the metres to its axis to the left of the head's point, the inside height of the
/// tube's chord there and its collar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Hole {
    pub opening: usize,
    pub across: f64,
    pub height: f64,
    pub collar: f64,
}

/// An opening's decision: every kind that fits in the order of the rungs, the part of the end (the first), the rung
/// that decided it, and the numbers where the opening is the first hole of a record.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct End {
    pub kinds: Vec<Kind>,
    pub kind: Option<Kind>,
    pub rung: &'static str,
    pub numbers: Option<Numbers>,
}

/// The decisions of all openings, the inside half-width and the floor of each room (None with no part), and the counts.
pub(crate) struct Decided {
    pub ends: Vec<End>,
    pub half: Vec<f64>,
    pub floors: Vec<Option<Floor>>,
    pub metrics: Value,
}

/// The measure of the asks, with the open ends of all openings.
pub(crate) type Measure<'a> = dyn Fn(&[Ask], &[OpenEnd]) -> (Vec<Site>, usize) + 'a;

/// Two openings of other tubes that lead the same way within `pair_max_deg` and lie within `pair_max_m`, and their
/// metres apart.
struct Near {
    a: usize,
    b: usize,
    d: f64,
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
                    out.push(Near { a: i, b: k, d });
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

/// The shift that moves opening `nearer` in along its way onto the frame line of opening `far`, within `pair_max_m`,
/// half its chain and its own way; None where its way does not reach the line.
fn onto_line(
    openings: &[Opening],
    tubes: &[Tube],
    (nearer, far): (usize, usize),
    b: &obc_trailview_parts::Bake,
) -> Option<f64> {
    let (pf, df) = (openings[far].p, openings[far].dir);
    let o = &openings[nearer];
    let t = &tubes[o.tube];
    let most = (o.shift + b.pair_max_m).min(t.curve.len / 2.0).min(first_way_m(t, o.end));
    let off = |shift: f64| along_dir(pf, o.at(t, shift).p, df);
    let (mut lo, mut hi) = (o.shift, most.max(o.shift));
    if off(hi) < 0.0 {
        return None;
    }
    for _ in 0..40 {
        let mid = (lo + hi) / 2.0;
        if off(mid) < 0.0 {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Some(hi)
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
pub(crate) fn decide(
    tubes: &[Tube],
    openings: &mut [Opening],
    input: &Input,
    measure: &Measure,
    rooms: &Rooms,
) -> Decided {
    let b = &input.rules.bake;
    let n = openings.len();
    let mut rung: Vec<Option<&'static str>> = vec![None; n];
    let half: Vec<f64> = tubes.iter().map(|t| t.half).collect();
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
    // The opening at each end of each tube.
    let mut at_end: Vec<[Option<usize>; 2]> = vec![[None; 2]; tubes.len()];
    for (i, o) in openings.iter().enumerate() {
        at_end[o.tube][o.end].get_or_insert(i);
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
    // Any two near openings may share a frame line, so each one's row reaches the other where it is and where it moves
    // onto this one's line, and its stations reach as far in as it moves onto the other's line.
    for x in near.iter().filter(|x| free[x.a] && free[x.b]) {
        for (i, k) in [(x.a, x.b), (x.b, x.a)] {
            let (o, q) = (&openings[i], &openings[k]);
            let tk = &tubes[q.tube];
            let edge = half[q.tube] + b.shell_m + input.rules.groups[tk.group].wing_max_m;
            let on_line = onto_line(openings, tubes, (k, i), b).map(|shift| q.at(tk, shift).p);
            let r = &mut reach[i];
            for p in std::iter::once(q.p).chain(on_line) {
                let lat = along_dir(o.p, p, left_of(o.dir));
                if lat >= 0.0 {
                    r.1 = r.1.max(lat + edge);
                } else {
                    r.0 = r.0.max(edge - lat);
                }
            }
            r.2 = r.2.max(onto_line(openings, tubes, (i, k), b).map_or(0.0, |shift| shift - o.shift));
        }
    }
    let asked: Vec<usize> = (0..n).filter(|&i| free[i]).collect();
    let floors_of = |o: &Opening, len: usize| -> Vec<f64> {
        let t = &tubes[o.tube];
        let sign = if o.end == 0 { 1.0 } else { -1.0 };
        (0..len).map(|k| t.curve.at(o.s + sign * k as f64).0.z).collect()
    };
    let asks: Vec<Ask> = asked.iter().map(|&i| ask_of(i, openings, tubes, &half, &reach, input)).collect();
    let first = open_ends(openings, rooms);
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
    // Rung 2 reads the rooms of the crossings and the buildings of both ends of a tube.
    let mut crossings: Vec<Vec<f64>> = vec![Vec::new(); tubes.len()];
    let mut building = vec![false; tubes.len()];
    for (i, site) in sites.iter().enumerate() {
        if let Some(site) = site {
            crossings[openings[i].tube].extend(&site.rooms);
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
        let (r, k): (&'static str, Vec<Kind>) = if short && !crossings[o.tube].is_empty() {
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
    // The pairs, nearest first, each end in one pair at most: two hill ends of one family a wall apart across. A pair is
    // one head of two rooms, so each of them has a part or a seam at both of its ends.
    let mut mate: Vec<Option<usize>> = vec![None; n];
    let is_hill = |i: usize| hill.contains(&i);
    let whole: Vec<bool> = (at_end.iter().enumerate())
        .map(|(t, e)| (0..2).all(|k| rooms.seam_at(t, k) || e[k].is_some_and(|i| !kinds[i].is_empty())))
        .collect();
    for x in &near {
        let (a, c) = (x.a, x.b);
        let (ta, tc) = (openings[a].tube, openings[c].tube);
        if mate[a].is_some() || mate[c].is_some() || !whole[ta] || !whole[tc] {
            continue;
        }
        // The holes of a head lie side by side: rooms one over the other open each with its own head.
        let (o, q) = (&openings[a], &openings[c]);
        let side_by_side = along_dir(o.p, q.p, left_of(o.dir)).abs() - half[ta] - half[tc] >= 2.0 * b.shell_m - 0.01;
        if is_hill(a) && is_hill(c) && tubes[ta].rail == tubes[tc].rail && side_by_side {
            (mate[a], mate[c]) = (Some(c), Some(a));
        }
    }
    // Two openings whose frames would overlap share a head too: their rooms lie at one level and nearer than two
    // frames and shells.
    let design = &obc_trailview_parts::Rules::embedded().design;
    let overlap = |i: usize, k: usize| {
        let (o, q) = (&openings[i], &openings[k]);
        let gap = along_dir(o.p, q.p, left_of(o.dir)).abs() - half[o.tube] - half[q.tube];
        let level = !super::rooms::apart((0.0, q.floor - o.floor), (height[o.tube], height[q.tube]), input);
        !kinds[i].is_empty() && !kinds[k].is_empty() && level && gap < 2.0 * (design.frame_m + b.shell_m)
    };
    // One frame line for each group of openings that pairs and overlaps link: the line of the group's far opening,
    // which never moves. The openings of the group near it (`near`) move in onto it along their ways and read the
    // ground that it measured, whose row reaches them; the others form the next group, with each that stays. The far
    // opening is one whose line none of the openings near it lies past by more than `PAST_M`, the one near the most,
    // then the farthest in along the group's way. An opening whose way does not reach the line, or that lies past it,
    // stays; a pair whose two openings do not end on one line is no pair: each opening has a head of its own.
    let mut root: Vec<usize> = (0..n).collect();
    let find = |root: &mut Vec<usize>, mut i: usize| {
        while root[i] != i {
            root[i] = root[root[i]];
            i = root[i];
        }
        i
    };
    let links: HashSet<(usize, usize)> = (0..n)
        .filter_map(|i| mate[i].map(|m| (i.min(m), i.max(m))))
        .chain(near.iter().filter(|x| overlap(x.a, x.b)).map(|x| (x.a.min(x.b), x.a.max(x.b))))
        .collect();
    let mut grouped = vec![false; n];
    for &(i, k) in &links {
        let (ri, rk) = (find(&mut root, i), find(&mut root, k));
        root[ri.max(rk)] = ri.min(rk);
        (grouped[i], grouped[k]) = (true, true);
    }
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..n {
        let r = find(&mut root, i);
        groups.entry(r).or_default().push(i);
    }
    let close: HashSet<(usize, usize)> = near.iter().map(|x| (x.a.min(x.b), x.a.max(x.b))).collect();
    let near_to = |i: usize, k: usize| close.contains(&(i.min(k), i.max(k)));
    let mut moved: Vec<f64> = vec![0.0; n];
    let mut far_of: Vec<usize> = (0..n).collect();
    let (mut unpaired, mut farthest_move) = (0, 0.0f64);
    for list in groups.into_values() {
        let mut left = list;
        while left.len() > 1 {
            let way = left.iter().fold([0.0, 0.0], |w, &i| [w[0] + openings[i].dir[0], w[1] + openings[i].dir[1]]);
            let along = |i: usize| along_dir(openings[left[0]].p, openings[i].p, way);
            let past = |f: usize, i: usize| along_dir(openings[f].p, openings[i].p, openings[f].dir) > PAST_M;
            let members = |f: usize| left.iter().copied().filter(move |&i| i != f && near_to(f, i));
            let clear = |f: usize| !members(f).any(|i| past(f, i));
            let rank = |a: &usize, b: &usize| {
                members(*a).count().cmp(&members(*b).count()).then(along(*a).total_cmp(&along(*b)))
            };
            let far = (left.iter().copied().filter(|&f| clear(f)).max_by(rank))
                .or_else(|| left.iter().copied().max_by(rank))
                .unwrap_or(left[0]);
            let shifts: Vec<(usize, f64)> = members(far)
                .filter(|&i| !past(far, i))
                .filter_map(|i| onto_line(openings, tubes, (i, far), b).map(|shift| (i, shift)))
                .collect();
            for (i, shift) in shifts {
                let o = openings[i].clone();
                moved[i] = shift - o.shift;
                farthest_move = farthest_move.max(moved[i]);
                openings[i] = Opening { others: o.others, ..o.at(&tubes[o.tube], shift) };
                far_of[i] = far;
            }
            left.retain(|&i| i != far && far_of[i] == i);
        }
    }
    // The openings on each frame line that others moved onto, the far opening with them; and the openings of a group,
    // but of a pair, that share no line.
    let mut lines: BTreeMap<usize, usize> = BTreeMap::new();
    for i in (0..n).filter(|&i| far_of[i] != i) {
        *lines.entry(far_of[i]).or_insert(1) += 1;
    }
    let alone = (0..n).filter(|&i| grouped[i] && mate[i].is_none() && far_of[i] == i && !lines.contains_key(&i));
    let alone = alone.count();
    for i in 0..n {
        let Some(m) = mate[i].filter(|&m| m > i && far_of[i] != far_of[m]) else { continue };
        unpaired += 1;
        (mate[i], mate[m]) = (None, None);
    }
    // The numbers come from the ground with the stretches that the device opens: of each opening of a chain whose two
    // ends are built (`BUILT`) by their final kinds, at its final place. The openings near a stretch that the first
    // measure opened otherwise are measured again there. Where the ground no longer fits a kind (no cover, or cover
    // past the limit), the end drops it; a pair of hill ends with such an end is no pair. Dropping kinds commutes,
    // so the order of the ends does not matter. The built chains can grow too (a broken pair's mate can become an
    // arch), but a round goes on only where a list of kinds shrank in the round before: the rounds end within one more
    // than the count of all kinds.
    let before: Vec<Option<Kind>> = kinds.iter().map(|k| k.first().copied()).collect();
    let (mut remeasured, mut last): (usize, Option<Vec<OpenEnd>>) = (0, None);
    loop {
        let now: Vec<Option<Kind>> = kinds.iter().map(|k| k.first().copied()).collect();
        let opened = open_ends(&built_openings(openings, &now, rooms), rooms);
        if last.as_ref() == Some(&opened) {
            break;
        }
        let changed: Vec<[f64; 2]> = (first.iter().filter(|e| !opened.contains(e)))
            .chain(opened.iter().filter(|e| !first.contains(e)))
            .map(|e| [e.way.x as f64 / SQUARE, e.way.y as f64 / SQUARE])
            .collect();
        let again: Vec<usize> = (0..n)
            .filter(|&i| !kinds[i].is_empty() && changed.iter().any(|&p| distance(p, openings[i].p) <= AGAIN_M))
            .collect();
        if !again.is_empty() {
            let asks: Vec<Ask> = again.iter().map(|&i| ask_of(i, openings, tubes, &half, &reach, input)).collect();
            let (measured, more) = measure(&asks, &opened);
            cells += more;
            remeasured += again.len();
            for ((&i, site), a) in again.iter().zip(measured).zip(&asks) {
                let o = &openings[i];
                cover[i] = first_covered(&site, &floors_of(o, site.stations.len()), need(o.tube)).map(|k| k as f64);
                (sites[i], rights[i], moved[i]) = (Some(site), a.right, 0.0);
            }
        }
        for &i in &again {
            let g = &input.rules.groups[tubes[openings[i].tube].group];
            let d = cover[i];
            let hill = d.is_some_and(|d| d <= g.collar_max_m);
            let fits = |k: &Kind| match k {
                Kind::Arch => hill,
                Kind::Rock => d.is_some_and(|d| d <= g.collar_max_m.min(b.rock_cover_max_m)),
                Kind::Box | Kind::Free | Kind::Seam => true,
            };
            let was = kinds[i].first().copied();
            kinds[i].retain(|k| fits(k));
            if was != Some(Kind::Free) && kinds[i].first() == Some(&Kind::Free) {
                rung[i] = Some(if d.is_none() { "free: never covered" } else { "free: covered past the limit" });
            }
            if let Some(m) = mate[i].filter(|_| !hill) {
                (mate[i], mate[m]) = (None, None);
            }
        }
        last = Some(opened);
    }
    let refitted = (0..n).filter(|&i| kinds[i].first().copied() != before[i]).count();
    // The floor of each tube with a part or a seam, from opening to opening; an end with no opening holds the end
    // vertex. Its chord at an opening gives the opening its inside height.
    let spans: Vec<Option<[f64; 2]>> = (tubes.iter().zip(&at_end).enumerate())
        .map(|(r, (t, &[a, z]))| {
            let seam = (0..2).any(|e| rooms.seam_at(r, e));
            let with = seam || [a, z].into_iter().flatten().any(|i| !kinds[i].is_empty());
            let s0 = a.map_or(0.0, |i| openings[i].s);
            let s1 = z.map_or(t.curve.len, |i| openings[i].s);
            (with && s1 - s0 > 1e-6).then_some([s0, s1])
        })
        .collect();
    let (floors, floor_metrics) = floor::floors(rooms, &spans, (&half, &height), input);
    let end_height = |o: &Opening| floors[o.tube].as_ref().map_or(height[o.tube], |f| f.end_height(o.end));
    let heights: Vec<f64> = openings.iter().map(end_height).collect();
    // The numbers of each drawn opening.
    let mut ends: Vec<End> = Vec::with_capacity(n);
    // The kind of each opening: it decides a hole's collar and whether a head has a wall.
    let base: Vec<Option<Kind>> = kinds.iter().map(|k| k.first().copied()).collect();
    // A head: the openings on one frame line that a pair or an overlap links, from the right of the line. It has one
    // record, at its first.
    let mut head: Vec<usize> = (0..n).collect();
    for &(i, k) in links.iter().filter(|&&(i, k)| far_of[i] == far_of[k] && base[i].is_some() && base[k].is_some()) {
        let (ri, rk) = (find(&mut head, i), find(&mut head, k));
        head[ri.max(rk)] = ri.min(rk);
    }
    let mut heads: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..n {
        let r = find(&mut head, i);
        heads.entry(r).or_default().push(i);
    }
    let mut members: Vec<Vec<usize>> = vec![Vec::new(); n];
    for mut list in heads.into_values() {
        let (p, left) = (openings[far_of[list[0]]].p, left_of(openings[far_of[list[0]]].dir));
        list.sort_by(|&x, &y| {
            let (ax, ay) = (along_dir(p, openings[x].p, left), along_dir(p, openings[y].p, left));
            ax.total_cmp(&ay).then((openings[x].way_id, x).cmp(&(openings[y].way_id, y)))
        });
        let first = list[0];
        members[first] = list;
    }
    // A wing row ends at the metre that comes within a wall of another room or another head: per head, metres across
    // its frame line from the point of its far opening.
    let axes = super::rooms::Axes::new(tubes);
    let widest = tubes.iter().map(|t| t.half).fold(0.0, f64::max);
    let wall = 2.0 * b.shell_m;
    let blocked = |list: &[usize], far: usize, x: f64| {
        let o = &openings[far];
        let q = super::offset(o.p, left_of(o.dir), x);
        let own: Vec<usize> = list.iter().map(|&k| openings[k].tube).collect();
        let room = (axes.near(q, usize::MAX, widest + wall).iter())
            .any(|f| !own.contains(&f.tube) && f.d < half[f.tube] + wall - 0.01);
        let head = (0..n).filter(|k| !list.contains(k) && !kinds[*k].is_empty()).any(|k| {
            let p = &openings[k];
            along_dir(p.p, q, p.dir).abs() <= design.face_m
                && along_dir(p.p, q, left_of(p.dir)).abs() < half[p.tube] + b.shell_m + wall
        });
        room || head
    };
    for i in 0..n {
        let kind = kinds[i].first().copied();
        let m = Measured { sites: &sites, rights: &rights, cover: &cover, moved: &moved };
        let numbers = (kind.filter(|_| !members[i].is_empty())).map(|_| {
            let stop = |x: f64| blocked(&members[i], far_of[i], x);
            numbers(&members[i], &base, (far_of[i], &stop), &m, (openings, tubes), (&half, &heights), input)
        });
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
        "measured_again": remeasured,
        "pairs_that_do_not_reach": unpaired,
        "heads_of_three_holes_or_more": members.iter().filter(|m| m.len() > 2).count(),
        "openings_moved_onto_a_line": (0..n).filter(|&i| mate[i].is_none() && far_of[i] != i).count(),
        "openings_that_share_no_line": alone,
        "openings_per_frame_line": counts(lines.values().map(|k| k.to_string())),
        "farthest_move_onto_a_line_m": (farthest_move * 100.0).round() / 100.0,
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

/// The numbers of the head of `members`, from the right of its frame line (Plan 8), with the kind and the inside height
/// of each opening: its frame is the frame line of the opening farther in, which holds them all; a wing row ends
/// before a metre across that line that is `blocked`.
fn numbers(
    members: &[usize],
    base: &[Option<Kind>],
    (far, blocked): (usize, &dyn Fn(f64) -> bool),
    m: &Measured,
    (openings, tubes): (&[Opening], &[Tube]),
    (half, heights): (&[f64], &[f64]),
    input: &Input,
) -> Numbers {
    let Measured { sites, rights, cover, moved } = *m;
    let i = members[0];
    let b = &input.rules.bake;
    let o = &openings[i];
    let t = &tubes[o.tube];
    let g = &input.rules.groups[t.group];
    let floor = o.floor;
    let site = sites[far].as_ref().expect("a measured opening");
    let left_far = left_of(openings[far].dir);
    let axis = |k: usize| along_dir(openings[far].p, openings[k].p, left_far);
    let edge = |k: usize| half[openings[k].tube] + b.shell_m;
    let lo = members.iter().map(|&k| axis(k) - edge(k)).fold(f64::INFINITY, f64::min);
    let hi = members.iter().map(|&k| axis(k) + edge(k)).fold(f64::NEG_INFINITY, f64::max);
    let ground = |x: f64| site.across(rights[far], x);
    let frame: Vec<f64> = std::iter::once(lo)
        .chain((lo.ceil() as i64..=hi.floor() as i64).map(|x| x as f64))
        .chain([hi])
        .filter_map(ground)
        .collect();
    let highest = frame.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    // The wall holds its band over the higher crown of the frame's tubes, and fits the ground up to its cap over the
    // lower.
    let crowns = members.iter().map(|&k| openings[k].floor - floor + heights[k] + b.shell_m);
    let (low, high) = crowns.fold((f64::INFINITY, f64::NEG_INFINITY), |(l, h), c| (l.min(c), h.max(c)));
    let mut face = (high + b.band_m).max((highest - floor + b.coping_m).min(low + b.face_over_crown_max_m));
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
        while d <= g.wing_max_m && !blocked(from + sign * d) {
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
    // Each tube of a pair has its own collar, by the kind of its opening.
    let own = |k: usize| base[k].unwrap_or(Kind::Arch);
    let collars: Vec<f64> = (members.iter())
        .map(|&k| {
            let half_chain = tubes[openings[k].tube].curve.len / 2.0;
            match own(k) {
                Kind::Arch | Kind::Rock => collar_of(k).expect("an arch is covered within its collar"),
                Kind::Box => collar_of(k).unwrap_or(f64::INFINITY).min(half_chain),
                Kind::Free | Kind::Seam => collar_of(k).unwrap_or(f64::INFINITY).min(b.free_max_m).min(half_chain),
            }
        })
        .collect();
    // The record holds the wall's numbers but on a free end and on a pair of two rims.
    let walls = if members.len() > 1 {
        members.iter().any(|&k| !matches!(own(k), Kind::Rock | Kind::Free))
    } else {
        own(i) != Kind::Free
    };
    // The lowest ground under the frame, its wing walls and the collars, and the floors.
    let (from, to) = (lo - right.len() as f64, hi + left.len() as f64);
    let under = (from.ceil() as i64..=to.floor() as i64).filter_map(|x| ground(x as f64));
    let collar_ground = members.iter().zip(&collars).flat_map(|(&k, collar)| {
        let site = sites[k].as_ref().expect("a measured opening");
        (site.stations.iter())
            .skip(moved[k].round() as usize)
            .take(collar.ceil() as usize + 1)
            .flat_map(|st| st.iter().flatten().copied())
    });
    let floors = members.iter().map(|&k| openings[k].floor);
    let lowest = under.chain(collar_ground).chain(floors).fold(floor, f64::min);
    let holes = (members.iter().zip(&collars))
        .map(|(&k, &collar)| Hole { opening: k, across: axis(k) - axis(i), height: heights[k], collar })
        .collect();
    let numbers = if walls { (left, right) } else { (Vec::new(), Vec::new()) };
    let heading = openings[far].heading();
    Numbers { heading, holes, walls, face, left: numbers.0, right: numbers.1, foot: lowest - b.foot_m - floor }
}
