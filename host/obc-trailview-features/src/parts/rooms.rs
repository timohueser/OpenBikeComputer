//! Tunnel ways that come nearer than a wall are one room (Tunnel parts in the spec, Rooms): the runs of two arms, the
//! rooms of their lanes, each room's centre line and section, and one frame line for the lanes at each of its ends.

use std::collections::{BTreeMap, HashMap};

use serde_json::{json, Value};

use super::floor::Floor;
use super::{along_dir, distance, first_way_m, left_of, offset, Curve, Input, Opening, Point, Tube};
use crate::{degrees, metres};

/// A chord of an arm's axis: its arm, its ends in world units, their heights and metres along the arm.
struct Seg {
    tube: usize,
    a: [f64; 2],
    b: [f64; 2],
    z: [f64; 2],
    s: [f64; 2],
}

/// The axes of all arms or rooms by cells of `CELL_M`.
pub(crate) struct Axes {
    segs: Vec<Seg>,
    grid: HashMap<(i64, i64), Vec<usize>>,
    cell: f64,
    len: Vec<f64>,
}

/// The side of a cell: wider than the farthest reach of a test (two halves and a wall).
const CELL_M: f64 = 32.0;

/// Another axis near a point: its tube, the metres to it, its height and its metres along there, and its unit
/// direction there in world units.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Foot {
    pub tube: usize,
    pub d: f64,
    pub z: f64,
    pub s: f64,
    pub dir: [f64; 2],
}

impl Axes {
    pub fn new(tubes: &[Tube]) -> Axes {
        let mut segs = Vec::new();
        for (t, tube) in tubes.iter().enumerate() {
            let points: Vec<_> = tube.curve.stretches.iter().flat_map(|(_, ps)| ps.iter()).collect();
            for w in points.windows(2) {
                if w[1].s > w[0].s {
                    segs.push(Seg { tube: t, a: w[0].p, b: w[1].p, z: [w[0].z, w[1].z], s: [w[0].s, w[1].s] });
                }
            }
        }
        let cell = segs.first().map_or(1.0, |s| CELL_M / metres(s.a[1]));
        let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        let at = |v: f64| (v / cell).floor() as i64;
        for (i, s) in segs.iter().enumerate() {
            for x in at(s.a[0].min(s.b[0]))..=at(s.a[0].max(s.b[0])) {
                for y in at(s.a[1].min(s.b[1]))..=at(s.a[1].max(s.b[1])) {
                    grid.entry((x, y)).or_default().push(i);
                }
            }
        }
        Axes { segs, grid, cell, len: tubes.iter().map(|t| t.curve.len).collect() }
    }

    /// The axis of each tube but `own` within `reach` metres of p, nearest per tube; a foot beyond a tube's end does not
    /// count.
    pub fn near(&self, p: [f64; 2], own: usize, reach: f64) -> Vec<Foot> {
        let m = metres(p[1]);
        let (cx, cy) = ((p[0] / self.cell).floor() as i64, (p[1] / self.cell).floor() as i64);
        // The cells round p as far as the reach goes.
        let k = (reach / m / self.cell).ceil().max(1.0) as i64;
        let mut best: HashMap<usize, Foot> = HashMap::new();
        for x in cx - k..=cx + k {
            for y in cy - k..=cy + k {
                for &i in self.grid.get(&(x, y)).into_iter().flatten() {
                    let seg = &self.segs[i];
                    if seg.tube == own {
                        continue;
                    }
                    let d = [(seg.b[0] - seg.a[0]) * m, (seg.b[1] - seg.a[1]) * m];
                    let q = [(p[0] - seg.a[0]) * m, (p[1] - seg.a[1]) * m];
                    let l2 = d[0] * d[0] + d[1] * d[1];
                    let u = (q[0] * d[0] + q[1] * d[1]) / l2;
                    let first = seg.s[0] <= 1e-9;
                    let last = seg.s[1] >= self.len[seg.tube] - 1e-9;
                    if (u < 0.0 && first) || (u > 1.0 && last) {
                        continue;
                    }
                    let u = u.clamp(0.0, 1.0);
                    let dist = (q[0] - u * d[0]).hypot(q[1] - u * d[1]);
                    if dist > reach || best.get(&seg.tube).is_some_and(|f| f.d <= dist) {
                        continue;
                    }
                    let z = seg.z[0] + u * (seg.z[1] - seg.z[0]);
                    let s = seg.s[0] + u * (seg.s[1] - seg.s[0]);
                    let l = l2.sqrt();
                    best.insert(seg.tube, Foot { tube: seg.tube, d: dist, z, s, dir: [d[0] / l, d[1] / l] });
                }
            }
        }
        let mut out: Vec<Foot> = best.into_values().collect();
        out.sort_by_key(|f| f.tube);
        out
    }
}

/// Two spaces lie apart: their gap is a wall or more, within the bake's centimetre, or one lies clear over the other,
/// a crown's shell and a slab over its crown; `rise` is the height of the second's floor over the first's.
pub(crate) fn apart((gap, rise): (f64, f64), (ha, hb): (f64, f64), input: &Input) -> bool {
    let b = &input.rules.bake;
    let (wall, roof) = (2.0 * b.shell_m, b.shell_m + b.slab_m);
    gap >= wall - MERGE_M || rise >= ha + roof || -rise >= hb + roof
}

/// The bake's centimetre (`MERGE_M` of the floor pass).
const MERGE_M: f64 = 0.01;

/// The gap and the rise of tube b at its foot from metres s of tube a's axis: the metres between their spaces (each
/// tube's half to each side of its axis) and the height of b's floor over a's.
pub(crate) fn beside(tubes: &[Tube], (a, s): (usize, f64), foot: &Foot) -> (f64, f64) {
    let q = tubes[a].curve.at(s).0;
    (foot.d - tubes[a].half - tubes[foot.tube].half, foot.z - q.z)
}

/// A metre of a run: its metres along the first arm, the gap and the rise of the second, the second's metres along
/// at the foot, the cosine of the angle between the two axes, and the side of the second (1 to the left).
#[derive(Clone, Copy, Debug)]
struct Metre {
    s: f64,
    gap: f64,
    rise: f64,
    foot: f64,
    cos: f64,
    side: f64,
}

/// What a run of two arms is (the table of the spec's Rooms).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Run {
    Apart,
    Height,
    Cross,
    Room,
}

impl Run {
    fn name(self) -> &'static str {
        match self {
            Run::Apart => "apart",
            Run::Height => "over and under by height",
            Run::Cross => "over and under by the floor",
            Run::Room => "one room",
        }
    }
}

/// A run of two arms: the arms, its metres along each at its two ends, and what it is.
#[derive(Clone, Copy, Debug)]
struct RunOf {
    a: usize,
    b: usize,
    sa: [f64; 2],
    sb: [f64; 2],
    kind: Run,
}

/// The runs of two arms along which their gap stays under a wall, with holes shorter than `room_join_m` closed.
fn runs(arms: &[Tube], spans: &[Option<[f64; 2]>], input: &Input) -> Vec<RunOf> {
    let b = &input.rules.bake;
    let wall = 2.0 * b.shell_m;
    let axes = Axes::new(arms);
    let widest = arms.iter().map(|t| t.half).fold(0.0, f64::max);
    let mut metres: BTreeMap<(usize, usize), Vec<Metre>> = BTreeMap::new();
    for (a, arm) in arms.iter().enumerate() {
        let Some([s0, s1]) = spans[a] else { continue };
        let steps = (s1 - s0).ceil().max(1.0) as usize;
        for k in 0..=steps {
            let s = s0 + (s1 - s0) * k as f64 / steps as f64;
            let (q, dir) = (arm.curve.at(s).0, arm.curve.dir(s));
            for f in
                axes.near(q.p, a, arm.half + widest + wall).iter().filter(|f| f.tube > a && spans[f.tube].is_some())
            {
                let (gap, rise) = beside(arms, (a, s), f);
                if gap >= wall - MERGE_M {
                    continue;
                }
                let foot = arms[f.tube].curve.at(f.s).0.p;
                let side = along_dir(q.p, foot, left_of(dir)).signum();
                let cos = dir[0] * f.dir[0] + dir[1] * f.dir[1];
                metres.entry((a, f.tube)).or_default().push(Metre { s, gap, rise, foot: f.s, cos, side });
            }
        }
    }
    let mut out = Vec::new();
    for ((a, bt), list) in metres {
        let mut run: Vec<Metre> = Vec::new();
        for m in list.into_iter().map(Some).chain([None]) {
            if let (Some(m), Some(last)) = (m, run.last()) {
                if m.s - last.s <= b.room_join_m {
                    run.push(m);
                    continue;
                }
            }
            if !run.is_empty() {
                let kind = decide(&run, (&arms[a], &arms[bt]), input);
                let (first, last) = (run[0], run[run.len() - 1]);
                out.push(RunOf { a, b: bt, sa: [first.s, last.s], sb: [first.foot, last.foot], kind });
            }
            run = m.into_iter().collect();
        }
    }
    out
}

/// What a run is: apart where one lies clear over the other all along; else over and under by height where the two
/// axes lie side by side and the least rise leaves the lower its least height and a roof; over and under by the floor
/// where they cross; else one room.
fn decide(run: &[Metre], (a, b): (&Tube, &Tube), input: &Input) -> Run {
    let bake = &input.rules.bake;
    if run.iter().all(|m| apart((0.0, m.rise), (a.height, b.height), input)) {
        return Run::Apart;
    }
    let cos = bake.pair_max_deg.to_radians().cos();
    let along = run.iter().filter(|m| m.cos.abs() >= cos).count();
    let sides: Vec<f64> = run.iter().filter(|m| m.gap > -a.half.min(b.half)).map(|m| m.side).collect();
    let cross = sides.windows(2).any(|w| w[0] != w[1]);
    let side_by_side = 2 * along > run.len() && !cross;
    let least = run.iter().min_by(|x, y| x.rise.abs().total_cmp(&y.rise.abs())).expect("a run has a metre");
    let lower = if least.rise > 0.0 { a } else { b };
    let low = input.rules.groups[lower.group].least_height_m;
    if side_by_side && least.rise.abs() >= low + bake.shell_m + bake.slab_m {
        Run::Height
    } else if !side_by_side {
        Run::Cross
    } else {
        Run::Room
    }
}

/// A lane of a room: its arm, whether it runs the room's way, and per room end the opening of the arm there.
#[derive(Clone, Debug)]
pub(crate) struct Lane {
    pub arm: usize,
    pub forward: bool,
    pub ends: [usize; 2],
}

/// A lane at a vertex of a room's centre line: its axis's metres to the left of the room's axis and its half-width,
/// both in the metres of the ring there (along the ring's line, times its cosine), and its metres along its arm.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Place {
    pub off: f64,
    pub half: f64,
    pub s: f64,
}

/// The rooms: per room its tube (of a lone arm, that arm), its lanes from the right wall to the left (none for a lone
/// arm) and its section per vertex of its centre line (by the metres of the vertex along the line); the arms, and
/// their openings with those of the lanes moved onto their rooms' frame lines; and the counts.
pub(crate) struct Rooms {
    pub tubes: Vec<Tube>,
    pub lanes: Vec<Vec<Lane>>,
    pub table: Vec<Vec<(f64, Vec<Place>)>>,
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
        if self.lanes[o.tube].is_empty() {
            return vec![Way { way: o.way, id: o.way_id, shift: o.shift }];
        }
        let lanes = &self.lanes[o.tube];
        let mut out: Vec<(f64, Way)> = lanes
            .iter()
            .map(|l| {
                let j = l.ends[o.end];
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

/// The shift that moves arm opening o in along its way onto the frame line through p square to `dir`, within `most`
/// metres more, half its chain and its own way; None where its way does not reach the line.
pub(crate) fn onto(arm: &Tube, o: &Opening, (p, dir): ([f64; 2], [f64; 2]), most: f64) -> Option<f64> {
    let most = (o.shift + most).min(arm.curve.len / 2.0).min(first_way_m(arm, o.end));
    let off = |shift: f64| along_dir(p, o.at(arm, shift).p, dir);
    let (mut lo, mut hi) = (o.shift, most.max(o.shift));
    if off(hi) < 0.0 {
        return None;
    }
    if off(lo) >= 0.0 {
        return Some(lo);
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

/// Where the line through p along the unit direction d crosses arm `arm`'s axis nearest its metres `near`: the metres
/// along the arm there, and the metres from p along d.
fn crossing(arm: &Tube, p: [f64; 2], d: [f64; 2], near: f64) -> Option<(f64, f64)> {
    let m = metres(p[1]);
    let side = |q: [f64; 2]| ((q[0] - p[0]) * d[1] - (q[1] - p[1]) * d[0]) * m;
    let points: Vec<&Point> = arm.curve.stretches.iter().flat_map(|(_, ps)| ps.iter()).collect();
    let mut best: Option<(f64, f64)> = None;
    for w in points.windows(2) {
        let (x, y) = (side(w[0].p), side(w[1].p));
        if (x > 0.0 && y > 0.0) || (x < 0.0 && y < 0.0) || x == y {
            continue;
        }
        let u = x / (x - y);
        let s = w[0].s + u * (w[1].s - w[0].s);
        let q = [w[0].p[0] + u * (w[1].p[0] - w[0].p[0]), w[0].p[1] + u * (w[1].p[1] - w[0].p[1])];
        if best.is_none_or(|b| (s - near).abs() < (b.0 - near).abs()) {
            best = Some((s, along_dir(p, q, d)));
        }
    }
    best
}

/// The least height over the floor at which a room's outline (of its `section`, half-width `half` and spring
/// `spring` of its height) holds each lane's outline on its floor: per lane its metres across, its half-width, its
/// floor over the room's and its height.
pub(crate) fn hold(round: bool, half: f64, spring: f64, lanes: &[(f64, f64, f64, f64)], segments: usize) -> f64 {
    let mut need: f64 = 0.0;
    for &(off, h, up, height) in lanes {
        if !round {
            need = need.max(up + height);
            continue;
        }
        // The lane's outline: its walls up to its spring, then its arch, at its corners.
        let sp = spring * height;
        let corners = (0..=segments).map(|k| {
            let a = std::f64::consts::PI * k as f64 / segments as f64;
            [off - h * a.cos(), up + sp + (height - sp) * a.sin()]
        });
        for [x, y] in corners.chain([[off - h, up + sp], [off + h, up + sp]]) {
            let r = (x / half).clamp(-1.0, 1.0);
            need = need.max(y / (spring + (1.0 - spring) * (1.0 - r * r).sqrt()));
        }
    }
    need
}

/// The rooms of the arms: per one-room run whose ends reach the openings of both its arms on each side (within
/// `free_max_m` of one of them), the arms are lanes of one room; the lanes of a room open on one frame line at each
/// end, that of its opening that none lies past, the others moved in along their ways onto it.
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
            (arms[t].heights && s1 - s0 > 1e-6).then_some([s0, s1])
        })
        .collect();
    let found = runs(&arms, &spans, input);
    let mut counts: BTreeMap<&str, u64> = BTreeMap::new();
    for r in &found {
        *counts.entry(r.kind.name()).or_default() += 1;
    }
    // A run's end reaches the openings on its side where each arm opens there and one of them lies within
    // `free_max_m` of it.
    let mut root: Vec<usize> = (0..n).collect();
    let find = |root: &mut Vec<usize>, mut i: usize| {
        while root[i] != i {
            root[i] = root[root[i]];
            i = root[i];
        }
        i
    };
    let mut in_the_hill = Vec::new();
    for &RunOf { a, b: c, sa, sb: sc, .. } in found.iter().filter(|r| r.kind == Run::Room) {
        let (sa_span, sc_span) = (spans[a].unwrap(), spans[c].unwrap());
        let forward = sc[1] >= sc[0];
        let reach = |end: usize| {
            let ae = if end == 0 { sa[0] - sa_span[0] } else { sa_span[1] - sa[1] };
            let cend = if forward { end } else { 1 - end };
            let ce = if cend == 0 { sc[end] - sc_span[0] } else { sc_span[1] - sc[end] };
            let opens = at_end[a][end].is_some() && at_end[c][cend].is_some();
            opens && ae.abs().min(ce.abs()) <= b.free_max_m
        };
        if reach(0) && reach(1) {
            let (ra, rc) = (find(&mut root, a), find(&mut root, c));
            root[ra.max(rc)] = ra.min(rc);
        } else {
            in_the_hill.push(arms[a].curve.at((sa[0] + sa[1]) / 2.0).0.p);
        }
    }
    let mut sets: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for t in 0..n {
        let r = find(&mut root, t);
        sets.entry(r).or_default().push(t);
    }
    let mut out = Rooms {
        tubes: Vec::new(),
        lanes: Vec::new(),
        table: Vec::new(),
        arms: Vec::new(),
        arm_openings: Vec::new(),
        metrics: Value::Null,
    };
    let mut openings: Vec<Opening> = Vec::new();
    let (mut off_line, mut moves): (Vec<String>, Vec<(f64, [f64; 2])>) = (Vec::new(), Vec::new());
    let mut moved_openings = arm_openings.clone();
    let place = |p: [f64; 2]| {
        let [lat, lon] = degrees(p);
        format!("{lat:.5},{lon:.5}")
    };
    for (_, set) in sets {
        let room = (set.len() > 1).then(|| room_of(&set, &arms, &moved_openings, &at_end, input));
        match room {
            Some(Ok((tube, lanes, table, ends, moved))) => {
                for (i, o, m) in moved {
                    moves.push((m, moved_openings[i].p));
                    moved_openings[i] = o;
                }
                let r = out.tubes.len();
                for (end, mut o) in ends.into_iter().enumerate() {
                    (o.tube, o.end) = (r, end);
                    openings.push(o);
                }
                out.tubes.push(tube);
                out.lanes.push(lanes);
                out.table.push(table);
            }
            other => {
                if let Some(Err(p)) = other {
                    off_line.push(place(p));
                }
                for &t in &set {
                    let r = out.tubes.len();
                    for i in at_end[t].into_iter().flatten() {
                        openings.push(Opening { tube: r, ..arm_openings[i].clone() });
                    }
                    out.tubes.push(arms[t].clone());
                    out.lanes.push(Vec::new());
                    out.table.push(Vec::new());
                }
            }
        }
    }
    let far: Vec<String> =
        moves.iter().filter(|m| m.0 > b.free_max_m).map(|m| format!("{} {:.0} m", place(m.1), m.0)).collect();
    out.metrics = json!({
        "runs": counts,
        "rooms": out.lanes.iter().filter(|l| !l.is_empty()).count(),
        "runs_in_the_hill": in_the_hill.len(),
        "runs_in_the_hill_places": in_the_hill.iter().take(10).map(|&p| place(p)).collect::<Vec<_>>().join("; "),
        "lanes_off_the_line": off_line.len(),
        "lanes_off_the_line_places": off_line.join("; "),
        "lane_moves_over_free_max": far.len(),
        "lane_moves_over_free_max_places": far.join("; "),
        "farthest_lane_move_m": (moves.iter().map(|m| m.0).fold(0.0, f64::max) * 100.0).round() / 100.0,
    });
    out.arms = arms;
    out.arm_openings = moved_openings;
    (out, openings)
}

/// The room of a set of arms joined by one-room runs, each open at both ends: its tube along its centre line, its lanes
/// from the right wall to the left, its section per vertex of the centre line, its two openings, and each lane's
/// opening moved onto its frame line (its place among the arms' openings, the moved opening and the metres moved); the
/// place of a lane whose way does not reach the line where there is one.
#[allow(clippy::type_complexity)]
fn room_of(
    set: &[usize],
    arms: &[Tube],
    arm_openings: &[Opening],
    at_end: &[[Option<usize>; 2]],
    input: &Input,
) -> Result<(Tube, Vec<Lane>, Vec<(f64, Vec<Place>)>, [Opening; 2], Vec<(usize, Opening, f64)>), [f64; 2]> {
    let mut openings = arm_openings.to_vec();
    let (b, mitre) = (&input.rules.bake, obc_trailview_parts::Rules::embedded().design.mitre_max);
    // The base: the widest arm, then the lowest id. A lane runs the room's way where its first opening lies nearer the
    // base's first.
    let base = *set
        .iter()
        .max_by(|&&x, &&y| arms[x].half.total_cmp(&arms[y].half).then(arms[y].widest.cmp(&arms[x].widest)))
        .unwrap();
    let opening = |t: usize, e: usize| at_end[t][e].expect("a lane opens at both ends");
    let first = openings[opening(base, 0)].p;
    let mut lanes: Vec<Lane> = set
        .iter()
        .map(|&t| {
            let forward = distance(openings[opening(t, 0)].p, first) <= distance(openings[opening(t, 1)].p, first);
            let ends = if forward { [opening(t, 0), opening(t, 1)] } else { [opening(t, 1), opening(t, 0)] };
            Lane { arm: t, forward, ends }
        })
        .collect();
    // Per end the frame line: of the opening that none lies past by more than `PAST_M`, then the farthest in.
    let mut moves = Vec::new();
    let mut fars = [0; 2];
    for (end, far_at) in fars.iter_mut().enumerate() {
        let list: Vec<usize> = lanes.iter().map(|l| l.ends[end]).collect();
        let way = list.iter().fold([0.0, 0.0], |w, &i| [w[0] + openings[i].dir[0], w[1] + openings[i].dir[1]]);
        let along = |i: usize| along_dir(openings[list[0]].p, openings[i].p, way);
        let past =
            |f: usize, i: usize| along_dir(openings[f].p, openings[i].p, openings[f].dir) > obc_trailview_parts::PAST_M;
        let clear: Vec<usize> = list.iter().copied().filter(|&f| !list.iter().any(|&i| past(f, i))).collect();
        let pick = if clear.is_empty() { &list } else { &clear };
        let far = pick.iter().copied().max_by(|&x, &y| along(x).total_cmp(&along(y))).unwrap();
        *far_at = far;
        let (pf, df) = (openings[far].p, openings[far].dir);
        for (k, &i) in list.iter().enumerate().filter(|&(_, &i)| i != far) {
            let arm = &arms[lanes[k].arm];
            let shift = onto(arm, &openings[i], (pf, df), f64::INFINITY).ok_or(openings[i].p)?;
            let o = openings[i].clone();
            openings[i] = Opening { others: o.others, ..o.at(arm, shift) };
            moves.push((i, openings[i].clone(), shift - o.shift));
        }
    }
    // The places of the lanes on a line through p with the unit direction `left`, and the metres to the left of p of
    // the middle between their outermost edges: per lane its metres along `left` and its half-width along it.
    let across = |p: [f64; 2], left: [f64; 2], normal: [f64; 2], near: &[f64]| -> Option<Vec<(f64, f64, f64)>> {
        (lanes.iter().zip(near))
            .map(|(l, &s)| {
                let arm = &arms[l.arm];
                let (sl, t) = crossing(arm, p, left, s)?;
                let d = arm.curve.dir(sl);
                let cos = (d[0] * normal[0] + d[1] * normal[1]).abs().max(1.0 / mitre);
                Some((t, arm.half / cos, sl))
            })
            .collect()
    };
    let middle = |places: &[(f64, f64, f64)]| {
        let lo = places.iter().map(|p| p.0 - p.1).fold(f64::INFINITY, f64::min);
        let hi = places.iter().map(|p| p.0 + p.1).fold(f64::NEG_INFINITY, f64::max);
        (lo + hi) / 2.0
    };
    // The centre line: on each frame line, and at each vertex of the base and the foot of each vertex of another lane
    // between them, the middle between the outermost edges of the lanes; its vertices `vertex_gap_m` apart at least.
    let span = |l: &Lane| [openings[l.ends[0]].s, openings[l.ends[1]].s];
    let base_arm = &arms[base];
    let [s0, s1] = span(&lanes[set.iter().position(|&t| t == base).unwrap()]);
    let mut samples: Vec<f64> = base_arm.curve.stretches.iter().flat_map(|(_, ps)| ps.iter().map(|q| q.s)).collect();
    for l in lanes.iter().filter(|l| l.arm != base) {
        let [a, z] = span(l);
        let (lo, hi) = (a.min(z), a.max(z));
        // The foot on the base of each vertex of the lane: where the base crosses the lane's normal there.
        for q in arms[l.arm].curve.stretches.iter().flat_map(|(_, ps)| ps.iter()).filter(|q| q.s > lo && q.s < hi) {
            if let Some((s, _)) = crossing(base_arm, q.p, left_of(arms[l.arm].curve.dir(q.s)), s0) {
                samples.push(s);
            }
        }
    }
    samples.retain(|&s| s > s0 && s < s1);
    samples.sort_by(f64::total_cmp);
    let near_of = |s: f64| -> Vec<f64> {
        lanes
            .iter()
            .map(|l| {
                let [a, z] = span(l);
                let u = (s - s0) / (s1 - s0).max(f64::MIN_POSITIVE);
                a + (z - a) * u
            })
            .collect()
    };
    let mut points: Vec<([f64; 2], f64)> = Vec::new();
    let ends_of = |end: usize| {
        let far = &openings[fars[end]];
        let sign = if end == 0 { 1.0 } else { -1.0 };
        let left = left_of(far.dir).map(|v| v * sign);
        let places: Vec<(f64, f64, f64)> = (lanes.iter())
            .map(|l| {
                let o = &openings[l.ends[end]];
                let d = o.dir;
                let cos = (d[0] * far.dir[0] + d[1] * far.dir[1]).abs().max(1.0 / mitre);
                (along_dir(far.p, o.p, left), arms[l.arm].half / cos, o.s)
            })
            .collect();
        (offset(far.p, left, middle(&places)), left)
    };
    let (start, start_left) = ends_of(0);
    let (stop, stop_left) = ends_of(1);
    points.push((start, s0));
    for &s in &samples {
        let (q, d) = (base_arm.curve.at(s).0.p, base_arm.curve.dir(s));
        let Some(places) = across(q, left_of(d), d, &near_of(s)) else { continue };
        let c = offset(q, left_of(d), middle(&places));
        let last = points[points.len() - 1].0;
        if distance(last, c) >= b.vertex_gap_m && distance(c, stop) >= b.vertex_gap_m {
            points.push((c, s));
        }
    }
    points.push((stop, s1));
    // The section at each vertex, on the ring there: the frame line at an end, else the mitre of the two chords.
    let n = points.len();
    let mut table: Vec<Vec<(f64, f64, f64)>> = Vec::with_capacity(n);
    let mut len = 0.0;
    let mut metres_at = Vec::with_capacity(n);
    for i in 0..n {
        if i > 0 {
            len += distance(points[i - 1].0, points[i].0);
        }
        metres_at.push(len);
        let row = if i == 0 || i + 1 == n {
            let (end, left) = if i == 0 { (0, start_left) } else { (1, stop_left) };
            let far = &openings[fars[end]];
            let (p, _) = points[i];
            (lanes.iter())
                .map(|l| {
                    let o = &openings[l.ends[end]];
                    let cos = (o.dir[0] * far.dir[0] + o.dir[1] * far.dir[1]).abs().max(1.0 / mitre);
                    (along_dir(p, o.p, left), arms[l.arm].half / cos, o.s)
                })
                .collect()
        } else {
            let unit = |a: [f64; 2], b: [f64; 2]| {
                let d = [b[0] - a[0], b[1] - a[1]];
                let l = d[0].hypot(d[1]);
                [d[0] / l, d[1] / l]
            };
            let (din, dout) = (unit(points[i - 1].0, points[i].0), unit(points[i].0, points[i + 1].0));
            let sum = [din[0] + dout[0], din[1] + dout[1]];
            let l = sum[0].hypot(sum[1]);
            let m = [sum[0] / l, sum[1] / l];
            let cos = ((m[0] * din[0] + m[1] * din[1] + m[0] * dout[0] + m[1] * dout[1]) / 2.0).max(1.0 / mitre);
            let near = table[i - 1].iter().map(|p| p.2).collect::<Vec<f64>>();
            let Some(places) = across(points[i].0, left_of(m), m, &near) else { return Err(points[i].0) };
            places.into_iter().map(|(t, h, s)| (t * cos, h * cos, s)).collect()
        };
        table.push(row);
    }
    // The lanes from the right wall to the left, by their places at the first end.
    let mut order: Vec<usize> = (0..lanes.len()).collect();
    order.sort_by(|&x, &y| table[0][x].0.total_cmp(&table[0][y].0));
    lanes = order.iter().map(|&k| lanes[k].clone()).collect();
    let table: Vec<(f64, Vec<Place>)> = (metres_at.iter().zip(&table))
        .map(|(&m, row)| (m, order.iter().map(|&k| Place { off: row[k].0, half: row[k].1, s: row[k].2 }).collect()))
        .collect();
    let half = table.iter().flat_map(|(_, row)| row.iter().map(|p| p.off.abs() + p.half)).fold(0.0, f64::max);
    // The tube of the room: its centre line at the least height of its lanes, as wide as they need and as high as
    // their outlines need at the heights of the road solver.
    let z_of =
        |row: &[Place]| -> Vec<f64> { (lanes.iter().zip(row)).map(|(l, p)| arms[l.arm].curve.at(p.s).0.z).collect() };
    let design = &obc_trailview_parts::Rules::embedded().design;
    let mut height: f64 = 0.0;
    let mut curve_points = Vec::with_capacity(n);
    for ((m, row), (p, s_base)) in table.iter().zip(&points) {
        let z = z_of(row);
        let low = z.iter().copied().fold(f64::INFINITY, f64::min);
        let outlines: Vec<(f64, f64, f64, f64)> = (lanes.iter().zip(row).zip(&z))
            .map(|((l, p), &zl)| (p.off, p.half, zl - low, arms[l.arm].height))
            .collect();
        height = height.max(hold(true, half, design.spring, &outlines, design.arch_segments));
        curve_points.push(Point { p: *p, z: low, s: *m, along: base_arm.curve.at(*s_base).0.along });
    }
    let base_lane = &arms[base];
    let mut ids: Vec<u32> = lanes.iter().flat_map(|l| arms[l.arm].ids.iter().copied()).collect();
    ids.sort_unstable();
    ids.dedup();
    let tube = Tube {
        curve: Curve { stretches: vec![(base_lane.widest, curve_points)], len },
        ids,
        heights: true,
        half,
        height,
        ..base_lane.clone()
    };
    let room_opening = |end: usize| {
        let far = &openings[fars[end]];
        let (s, p) = if end == 0 { (0.0, start) } else { (len, stop) };
        Opening { shift: 0.0, s, p, floor: tube.curve.at(s).0.z, ..far.clone() }
    };
    let ends = [room_opening(0), room_opening(1)];
    Ok((tube, lanes, table, ends, moves))
}

/// Per two rooms with floors that come beside each other (`beside` and `apart`), the first place where they do: each
/// room's centre line read every metre against the axes near it, on the final floors, halves and heights.
pub(crate) fn touching(rooms: &Rooms, floors: &[Option<Floor>], half: &[f64], input: &Input) -> Vec<[f64; 2]> {
    let wall = 2.0 * input.rules.bake.shell_m;
    // The rooms at their floors, with their halves; a room with no floor is none.
    let tubes: Vec<Tube> = (rooms.tubes.iter().zip(floors).zip(half))
        .map(|((t, f), &half)| {
            let mut t = t.clone();
            let Some(f) = f else {
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
    let mut out: BTreeMap<(usize, usize), [f64; 2]> = BTreeMap::new();
    for (a, t) in tubes.iter().enumerate().filter(|(_, t)| !t.curve.stretches.is_empty()) {
        let steps = t.curve.len.ceil().max(1.0) as usize;
        for k in 0..=steps {
            let s = t.curve.len * k as f64 / steps as f64;
            let p = t.curve.at(s).0.p;
            for f in axes.near(p, a, t.half + widest + wall).iter().filter(|f| f.tube > a) {
                if !apart(beside(&tubes, (a, s), f), (height(a, s), height(f.tube, f.s)), input) {
                    out.entry((a, f.tube)).or_insert(p);
                }
            }
        }
    }
    out.into_values().collect()
}
