//! Tunnel ways that come nearer than a wall are one room (Tunnel parts in the spec, Rooms): the runs of two arms, the
//! rooms of their lanes, each room's centre line and section, and one frame line for the lanes at each of its ends.

use std::collections::{BTreeMap, HashMap};

use serde_json::{json, Value};

use super::floor::Floor;
use super::ladder::Kind;
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

/// A run of two arms: the arms, its metres along each at its two ends, what it is, and the rise of the second where
/// the two lie nearest in height.
#[derive(Clone, Copy, Debug)]
struct RunOf {
    a: usize,
    b: usize,
    sa: [f64; 2],
    sb: [f64; 2],
    kind: Run,
    rise: f64,
}

/// The runs of two arms along which their gap stays under a wall, with holes shorter than `room_join_m` closed.
fn runs(arms: &[Tube], spans: &[Option<[f64; 2]>], input: &Input) -> Vec<RunOf> {
    let b = &input.rules.bake;
    let wall = 2.0 * b.shell_m;
    let mitre = obc_trailview_parts::Rules::embedded().design.mitre_max;
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
                // On a line square to either axis the other's half is its own over the cosine of the angle between
                // them: the run goes on until the two lie a wall apart along it too, so the holes of a seam do.
                let cos = dir[0] * f.dir[0] + dir[1] * f.dir[1];
                let spread = (arm.half + arms[f.tube].half) * (1.0 / cos.abs().max(1.0 / mitre) - 1.0);
                if gap - spread >= wall - MERGE_M {
                    continue;
                }
                let foot = arms[f.tube].curve.at(f.s).0.p;
                let side = along_dir(q.p, foot, left_of(dir)).signum();
                metres.entry((a, f.tube)).or_default().push(Metre { s, gap, rise, foot: f.s, cos, side });
            }
        }
    }
    // The metres along arm a of each vertex that the two arms share where one of them ends (a fork or a T): they meet
    // there at one level.
    let shared = |a: usize, c: usize| {
        let points = |t: &Tube| -> Vec<(f64, [u64; 2])> {
            (t.curve.stretches.iter()).flat_map(|(_, ps)| ps.iter().map(|q| (q.s, q.p.map(f64::to_bits)))).collect()
        };
        let ends = |t: &Tube| [0.0, t.curve.len].map(|s| t.curve.at(s).0.p.map(f64::to_bits));
        let (mine, theirs) = (points(&arms[a]), points(&arms[c]));
        let (my_ends, their_ends) = (ends(&arms[a]), ends(&arms[c]));
        let on = |k: &[u64; 2]| theirs.iter().any(|q| q.1 == *k);
        let joins = |q: &&(f64, [u64; 2])| (my_ends.contains(&q.1) && on(&q.1)) || their_ends.contains(&q.1);
        mine.iter().filter(joins).map(|q| q.0).collect::<Vec<f64>>()
    };
    let mut out = Vec::new();
    for ((a, bt), list) in metres {
        let at_vertex = shared(a, bt);
        let reach = arms[a].half + arms[bt].half + wall;
        let mut run: Vec<Metre> = Vec::new();
        for m in list.into_iter().map(Some).chain([None]) {
            if let (Some(m), Some(last)) = (m, run.last()) {
                if m.s - last.s <= b.room_join_m {
                    run.push(m);
                    continue;
                }
            }
            if !run.is_empty() {
                let (first, last) = (run[0], run[run.len() - 1]);
                let junction = at_vertex.iter().any(|&e| e >= first.s - reach && e <= last.s + reach);
                let kind = decide(&run, (&arms[a], &arms[bt]), junction, input);
                let rise = run.iter().map(|m| m.rise).min_by(|x, y| x.abs().total_cmp(&y.abs())).unwrap_or(0.0);
                out.push(RunOf { a, b: bt, sa: [first.s, last.s], sb: [first.foot, last.foot], kind, rise });
            }
            run = m.into_iter().collect();
        }
    }
    out
}

/// What a run is: apart where one lies clear over the other all along; else over and under by height where the two
/// axes lie side by side and the least rise leaves the lower its least height and a roof; over and under by the floor
/// where they cross; else one room. A run that reaches a vertex of both arms where one of them ends (`junction`) lies
/// at one level there: where the two do not lie side by side, it changes nothing.
fn decide(run: &[Metre], (a, b): (&Tube, &Tube), junction: bool, input: &Input) -> Run {
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
    } else if !side_by_side && junction {
        Run::Apart
    } else if !side_by_side {
        Run::Cross
    } else {
        Run::Room
    }
}

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
    /// Per run of over and under: the room over, the room under, and whether only the section of the room under goes
    /// lower (the two lie side by side).
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

    /// Per room whether it draws as parts: it is not `out`, each of its ends is a whole seam or of a built kind (`kind`
    /// of its opening). The rooms of one seam and the rooms of one arm draw together or not at all.
    pub fn built(&self, kind: &dyn Fn(usize, usize) -> Option<Kind>, out: &[bool]) -> Vec<bool> {
        let n = self.tubes.len();
        let broken = |t: usize| (0..2).any(|e| matches!(self.bounds[t][e], Bound::Seam(s) if !self.seams[s].whole));
        let own = |t: usize| {
            !out[t] && !broken(t) && (0..2).all(|e| self.seam_at(t, e) || obc_trailview_parts::built(&[kind(t, e)]))
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

/// The least height over the floor at which a room's outline (round or of box section, half-width `half`, its arch in
/// `segments` springing at the share `spring` of its height) holds the outline of each lane on its floor: per lane its
/// metres across, its half-width, its floor over the room's and its height; a lane's arch in `arch_segments`. A
/// round room is read at the corners of its outline, as the device draws it.
pub(crate) fn hold(
    round: bool,
    half: f64,
    spring: f64,
    lanes: &[(f64, f64, f64, f64)],
    (segments, arch_segments): (usize, usize),
) -> f64 {
    if !round {
        return lanes.iter().map(|l| l.2 + l.3).fold(0.0, f64::max);
    }
    // The room's outline of height 1 from its right spring to its left, and its top at x.
    let arch = |n: usize, half: f64, sp: f64, h: f64| -> Vec<[f64; 2]> {
        (0..=n)
            .map(|k| {
                let a = std::f64::consts::PI * k as f64 / n as f64;
                [-half * a.cos(), sp + (h - sp) * a.sin()]
            })
            .collect()
    };
    let unit = arch(segments, half, spring, 1.0);
    let top = |x: f64| {
        let x = x.clamp(-half, half);
        let k = unit.partition_point(|q| q[0] < x).clamp(1, unit.len() - 1);
        let (p, q) = (unit[k - 1], unit[k]);
        p[1] + (q[1] - p[1]) * ((x - p[0]) / (q[0] - p[0]).max(f64::MIN_POSITIVE)).clamp(0.0, 1.0)
    };
    let mut need: f64 = 0.0;
    for &(off, h, up, height) in lanes {
        let sp = spring * height;
        let corners = arch(arch_segments, h, sp, height).into_iter().chain([[-h, 0.0], [h, 0.0]]);
        for [x, y] in corners {
            need = need.max((up + y.max(sp.min(y))) / top(off + x));
        }
    }
    need
}

/// A one-room run with its ends moved: its two arms and its metres along each at its two ends.
#[derive(Clone, Copy, Debug)]
struct Joined {
    a: usize,
    c: usize,
    sa: [f64; 2],
    sc: [f64; 2],
}

/// The metres along arm `to` at the foot of the point at metres s along arm `from`: where `to` crosses the square
/// through that point, nearest `near`.
fn foot(arms: &[Tube], (from, s): (usize, f64), to: usize, near: f64) -> Option<f64> {
    let (p, d) = (arms[from].curve.at(s).0.p, arms[from].curve.dir(s));
    crossing(&arms[to], p, left_of(d), near).map(|x| x.0)
}

/// A room and one of its ends.
type RoomEnd = (usize, usize);

/// A seam: the room before the line and its end there, the rooms behind it with their ends, the line's point (on the
/// base of the room before) and its heading from that room into the others, per room behind the line its hole's metres
/// across from the end of the room before and its half-width along the line, and whether the row of the room before
/// holds a hole for each room behind it.
#[derive(Clone, Debug)]
pub(crate) struct Seam {
    pub parent: (usize, usize),
    pub children: Vec<(usize, usize)>,
    pub p: [f64; 2],
    pub dir: [f64; 2],
    pub holes: Vec<(f64, f64)>,
    pub whole: bool,
}

/// Where a room behind a seam starts: its hole's point on the line, and per lane its arm, its metres across to the left
/// of the seam's heading from that point, its half-width along the line and its metres along its arm.
#[derive(Clone, Debug)]
struct Start {
    p: [f64; 2],
    lanes: Vec<(usize, f64, f64, f64)>,
}

/// The holes of a seam in the row of the room before it (per lane its arm, its metres across to the left of the
/// heading, its half-width along the line and its metres along its arm), one per room behind the line (by the arms of
/// its lanes): from the right side of its first lane to the left side of its last, a side between two lanes in the
/// middle between them, cut back by `shell_m` from a side that it shares with another hole. Per hole its metres across
/// (to the centimetre), its half-width (down to the centimetre) and its lanes, across from its middle, each half-width
/// kept inside the hole. None where a room's lanes are not side by side in the row, or a hole has no width.
fn holes_of(row: &[(usize, f64, f64, f64)], children: &[Vec<usize>], shell: f64) -> Option<Vec<(f64, f64, Start)>> {
    let mut row = row.to_vec();
    row.sort_by(|x, y| x.1.total_cmp(&y.1));
    let owner: Vec<Option<usize>> = row.iter().map(|l| children.iter().position(|c| c.contains(&l.0))).collect();
    let right =
        |k: usize| if k == 0 { row[0].1 - row[0].2 } else { (row[k - 1].1 + row[k - 1].2 + row[k].1 - row[k].2) / 2.0 };
    let left = |k: usize| if k + 1 == row.len() { row[k].1 + row[k].2 } else { right(k + 1) };
    let mut out = Vec::new();
    for (c, arms) in children.iter().enumerate() {
        let at: Vec<usize> = (0..row.len()).filter(|&k| owner[k] == Some(c)).collect();
        if at.len() != arms.len() || at.is_empty() || at.windows(2).any(|w| w[1] != w[0] + 1) {
            return None;
        }
        let (first, last) = (at[0], at[at.len() - 1]);
        let other = |k: usize| owner[k].is_some_and(|o| o != c);
        let lo = right(first) + if first > 0 && other(first - 1) { shell } else { 0.0 };
        let hi = left(last) - if last + 1 < row.len() && other(last + 1) { shell } else { 0.0 };
        let across = ((lo + hi) / 2.0 * 100.0).round() / 100.0;
        let half = (((across - lo).min(hi - across)) * 100.0 + 1e-4).floor() / 100.0;
        if half <= 0.0 {
            return None;
        }
        let lanes = at
            .iter()
            .map(|&k| {
                let (arm, x, h, s) = row[k];
                let off = x - across;
                (arm, off, h.min(half - off).min(half + off).max(0.0), s)
            })
            .collect();
        out.push((across, half, Start { p: [0.0, 0.0], lanes }));
    }
    Some(out)
}

/// An end of a room: of its lanes' openings, at a seam's line, or none (a lane ends where its tunnel goes on with no
/// opening, or the lanes end in other ways).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Bound {
    Open,
    Seam(usize),
    None,
}

/// The rooms of the arms. The ends of each one-room run move: within `free_max_m` of an opening of one of its arms, to
/// the openings where both arms open on that side, else in, to `free_max_m` from that opening. Each arm is cut where a
/// run ends off its openings, and a cut inside a run cuts the other arm at its foot, until no cut is new. The stretches
/// between the cuts are lanes, and lanes that a run joins are one room. Where the rooms on one side of the cuts of one
/// place are one room, that place is a seam on a line square to that room's widest lane. The lanes of a room open on
/// one frame line at each end with openings.
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
    let found = runs(&arms, &live, input);
    let mut counts: BTreeMap<&str, u64> = BTreeMap::new();
    for r in &found {
        *counts.entry(r.kind.name()).or_default() += 1;
    }
    // The ends of the one-room runs.
    let mut joined: Vec<Joined> = Vec::new();
    let mut dropped = 0;
    for r in found.iter().filter(|r| r.kind == Run::Room) {
        let (a, c) = (r.a, r.b);
        let (pa, pc) = (spans[a].unwrap(), spans[c].unwrap());
        let forward = r.sb[1] >= r.sb[0];
        let (mut sa, mut sc) = (r.sa, r.sb);
        // Per arm and side, the metres along it of its opening there; the run's end at `end` lies `into` from the run.
        let (into, opening) = (
            |end: usize| if end == 0 { -1.0 } else { 1.0 },
            |t: usize, side: usize| at_end[t][side].map(|_| spans[t].unwrap()[side]),
        );
        for end in 0..2 {
            let cend = if forward { end } else { 1 - end };
            let near = |x: f64, o: Option<f64>| o.filter(|o| (x - o).abs() <= b.free_max_m);
            let (own_a, own_c) = (opening(a, end), opening(c, cend));
            let (far_a, far_c) = (near(sa[end], opening(a, 1 - end)), near(sc[end], opening(c, 1 - cend)));
            let near_own = near(sa[end], own_a).is_some() || near(sc[end], own_c).is_some();
            // The run reaches an opening (the two do not part before it) where it ends within a wall and a vertex
            // gap of it.
            let reach = 2.0 * b.shell_m + b.vertex_gap_m;
            let reaches = |x: f64, o: Option<f64>| o.is_some_and(|o| (x - o).abs() <= reach);
            if let (true, Some(oa), Some(oc)) = (near_own, own_a, own_c) {
                // To the openings on its side where the run reaches one; else the two part in the hill before their
                // openings, and the run ends there.
                if reaches(sa[end], own_a) || reaches(sc[end], own_c) {
                    (sa[end], sc[end]) = (oa, oc);
                }
            } else if let Some(o) = near(sa[end], own_a).or(far_a) {
                // In from that opening, `free_max_m` from it.
                let toward = if near(sa[end], own_a).is_some() { -into(end) } else { into(end) };
                sa[end] = o + toward * b.free_max_m;
                let guess = sc[0] + (sc[1] - sc[0]) * (sa[end] - r.sa[0]) / (r.sa[1] - r.sa[0]).max(f64::MIN_POSITIVE);
                sc[end] = foot(&arms, (a, sa[end]), c, guess).unwrap_or(sc[end]);
            } else if let Some(o) = near(sc[end], own_c).or(far_c) {
                let toward = if near(sc[end], own_c).is_some() { -into(cend) } else { into(cend) };
                sc[end] = o + toward * b.free_max_m;
                let guess = sa[0] + (sa[1] - sa[0]) * (sc[end] - r.sb[0]) / (r.sb[1] - r.sb[0]);
                sa[end] = foot(&arms, (c, sc[end]), a, guess).unwrap_or(sa[end]);
            }
            let [s0, s1] = pa;
            sa[end] = sa[end].clamp(s0, s1);
            let [c0, c1] = pc;
            sc[end] = sc[end].clamp(c0, c1);
        }
        if sa[1] - sa[0] < b.vertex_gap_m || (sc[1] - sc[0]).abs() < b.vertex_gap_m {
            dropped += 1;
            continue;
        }
        joined.push(Joined { a, c, sa, sc });
    }
    // The cuts of each arm, where a run ends off its openings; a cut inside a run cuts the other arm at its foot. A cut
    // within `vertex_gap_m` of another is that cut: the foot of a foot lies a little off where two arms are not
    // parallel, so the cuts end.
    let mut cuts: Vec<Vec<f64>> = vec![Vec::new(); n];
    let add = |cuts: &mut Vec<Vec<f64>>, t: usize, s: f64| {
        let [s0, s1] = spans[t].unwrap();
        let new = s > s0 + 1e-6 && s < s1 - 1e-6 && !cuts[t].iter().any(|x| (x - s).abs() < b.vertex_gap_m);
        if new {
            cuts[t].push(s);
        }
        new
    };
    for j in &joined {
        for end in 0..2 {
            add(&mut cuts, j.a, j.sa[end]);
            add(&mut cuts, j.c, j.sc[end]);
        }
    }
    let inside = |x: f64, [p, q]: [f64; 2]| x > p.min(q) + 1e-6 && x < p.max(q) - 1e-6;
    let mut rounds = 0;
    loop {
        let mut new = false;
        for j in &joined {
            let along = |x: f64, (from, to): ([f64; 2], [f64; 2])| {
                to[0] + (to[1] - to[0]) * (x - from[0]) / (from[1] - from[0])
            };
            for x in cuts[j.a].clone().into_iter().filter(|&x| inside(x, j.sa)) {
                if let Some(y) = foot(&arms, (j.a, x), j.c, along(x, (j.sa, j.sc))) {
                    new |= add(&mut cuts, j.c, y);
                }
            }
            for y in cuts[j.c].clone().into_iter().filter(|&y| inside(y, j.sc)) {
                if let Some(x) = foot(&arms, (j.c, y), j.a, along(y, (j.sc, j.sa))) {
                    new |= add(&mut cuts, j.a, x);
                }
            }
        }
        rounds += 1;
        if !new || rounds > 20 {
            break;
        }
    }
    // The stretches between the cuts, and their rooms.
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
    for j in &joined {
        for (k, (_, [s0, s1])) in stretches.iter().enumerate().take(first[j.a + 1]).skip(first[j.a]) {
            let m = (s0 + s1) / 2.0;
            if !inside(m, j.sa) {
                continue;
            }
            let guess = j.sc[0] + (j.sc[1] - j.sc[0]) * (m - j.sa[0]) / (j.sa[1] - j.sa[0]);
            let Some(other) = foot(&arms, (j.a, m), j.c, guess).and_then(|y| of(j.c, y)) else { continue };
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
    let mut rooms: Vec<(Vec<Lane>, [Bound; 2])> = Vec::new();
    let mut cut_ends: BTreeMap<(usize, u64), Vec<(usize, usize)>> = BTreeMap::new();
    for set in sets.into_values() {
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
                let forward = k == base || distance(p0, start) <= distance(p1, start);
                let span = if forward { [s0, s1] } else { [s1, s0] };
                let side = |s: f64| {
                    if (s - spans[t].unwrap()[0]).abs() < 1e-6 {
                        Some(0)
                    } else if (s - spans[t].unwrap()[1]).abs() < 1e-6 {
                        Some(1)
                    } else {
                        None
                    }
                };
                let ends = span.map(|s| side(s).and_then(|e| at_end[t][e]));
                Lane { arm: t, span, ends }
            })
            .collect();
        let r = rooms.len();
        let bounds = [0, 1].map(|end| {
            let kinds: Vec<Option<bool>> = (lanes.iter())
                .map(|l| {
                    let [s0, s1] = spans[l.arm].unwrap();
                    let at_span = (l.span[end] - s0).abs() < 1e-6 || (l.span[end] - s1).abs() < 1e-6;
                    if at_span {
                        l.ends[end].map(|_| true)
                    } else {
                        Some(false)
                    }
                })
                .collect();
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
    // The seams: the room ends that share a cut. One side of the line is one room, the room before it; the line lies
    // square to that room's base, through the base's cut.
    let mut group: BTreeMap<(usize, usize), usize> = BTreeMap::new();
    let mut groups: Vec<Vec<(usize, usize)>> = Vec::new();
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
    let mut seams: Vec<Seam> = Vec::new();
    let mut no_parent = Vec::new();
    for list in groups.into_iter().filter(|g| !g.is_empty()) {
        // The sides: a room lies after the cut where its stretch starts there, its arm led the way of the first room's
        // base there.
        let (r0, e0) = list[0];
        let l0 = &rooms[r0].0[0];
        let d = arms[l0.arm].curve.dir(l0.span[e0]);
        let side_of = |&(r, e): &(usize, usize)| {
            let l = &rooms[r].0[0];
            let after = l.span[1 - e] > l.span[e];
            let own = arms[l.arm].curve.dir(l.span[e]);
            after == (own[0] * d[0] + own[1] * d[1] >= 0.0)
        };
        let (after, before): (Vec<RoomEnd>, Vec<RoomEnd>) = list.iter().partition(|e| side_of(e));
        let (parent, children) = match (before.len(), after.len()) {
            (1, _) if after.len() > 1 || rooms[before[0].0].0.len() >= rooms[after[0].0].0.len() => (before[0], after),
            (_, 1) => (after[0], before),
            _ => {
                no_parent.push(arms[l0.arm].curve.at(l0.span[e0]).0.p);
                for &(r, e) in &list {
                    rooms[r].1[e] = Bound::None;
                }
                continue;
            }
        };
        let base = &rooms[parent.0].0[0];
        let at = base.span[parent.1];
        let (p, d) = (arms[base.arm].curve.at(at).0.p, arms[base.arm].curve.dir(at));
        // The heading leads from the room before into the others.
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
        seams.push(Seam { parent, children, p, dir, holes: Vec::new(), whole: true });
    }
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
            match holes_of(&row, &children, b.shell_m) {
                Some(list) => {
                    for (&child, (across, half, mut st)) in seam.children.iter().zip(list) {
                        st.p = offset(at, left_of([h.cos(), h.sin()]), across);
                        starts.insert(child, st);
                        out.seams[s].holes.push((across, half));
                    }
                }
                None => out.seams[s].whole = false,
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
    let far: Vec<String> =
        moves.iter().filter(|m| m.0 > b.free_max_m).map(|m| format!("{} {:.0} m", place(m.1), m.0)).collect();
    out.metrics = json!({
        "runs": counts,
        "runs_too_short_once_moved": dropped,
        "rooms": out.lanes.iter().filter(|l| l.len() > 1).count(),
        "seams": out.seams.len(),
        "cuts_without_one_room_on_a_side": no_parent.len(),
        "cuts_without_one_room_on_a_side_places": no_parent.iter().take(10).map(|&p| place(p)).collect::<Vec<_>>().join("; "),
        "lanes_off_the_line": off_line.len(),
        "lanes_off_the_line_places": off_line.join("; "),
        "lane_moves_over_free_max": far.len(),
        "lane_moves_over_free_max_places": far.join("; "),
        "farthest_lane_move_m": (moves.iter().map(|m| m.0).fold(0.0, f64::max) * 100.0).round() / 100.0,
    });
    // The runs of over and under between the rooms of their arms.
    let mut of_arm: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (r, lanes) in out.lanes.iter().enumerate() {
        for l in lanes {
            of_arm[l.arm].push(r);
        }
    }
    for run in found.iter().filter(|r| matches!(r.kind, Run::Height | Run::Cross)) {
        let (upper, lower) = if run.rise > 0.0 { (run.b, run.a) } else { (run.a, run.b) };
        for &u in &of_arm[upper] {
            for &t in of_arm[lower].iter().filter(|&&t| t != u) {
                out.over.push((u, t, run.kind == Run::Height));
            }
        }
    }
    out.over.sort_unstable_by_key(|o| (o.0, o.1, o.2));
    out.over.dedup();
    out.arms = arms;
    out.arm_openings = moved_openings;
    (out, openings)
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

/// The tube of a room of one lane started at its end `end` at the hole's point p: no other vertex of its first way
/// lies within `clear` metres of it.
fn started(t: &mut Tube, end: usize, p: [f64; 2], clear: f64) {
    let stretches = &mut t.curve.stretches;
    let k = if end == 0 { 0 } else { stretches.len() - 1 };
    let ps = &mut stretches[k].1;
    let first = if end == 0 { 0 } else { ps.len() - 1 };
    ps[first].p = p;
    let other = if end == 0 { ps.len() - 1 } else { 0 };
    let keep: Vec<bool> = (0..ps.len()).map(|i| i == first || i == other || distance(p, ps[i].p) >= clear).collect();
    let mut i = 0;
    ps.retain(|_| {
        i += 1;
        keep[i - 1]
    });
}

/// The tube of a stretch of an arm from metres s0 to s1: its points between, its metres from s0 on.
fn stretch(t: &Tube, [s0, s1]: [f64; 2]) -> Tube {
    let mut curve = Curve { stretches: Vec::new(), len: s1 - s0 };
    for (way, ps) in &t.curve.stretches {
        let (a, z) = (ps[0].s.max(s0), ps[ps.len() - 1].s.min(s1));
        if z - a <= 1e-9 {
            continue;
        }
        let mut points = vec![t.curve.at(a).0];
        points.extend(ps.iter().filter(|q| q.s > a && q.s < z).copied());
        points.push(t.curve.at(z).0);
        points.iter_mut().for_each(|q| q.s -= s0);
        curve.stretches.push((*way, points));
    }
    let mut ids: Vec<u32> = curve.stretches.iter().map(|w| w.0).collect();
    ids.dedup();
    Tube { curve, ids, ..t.clone() }
}

/// An end of a room on its line: the room's centre there, its left, and per lane its metres across, its half-width
/// along the line and its metres along its arm.
type EndRow = ([f64; 2], [f64; 2], Vec<(f64, f64, f64)>);

/// What `room_of` gives: the room's tube, its lanes from the right wall to the left, its section per vertex of its
/// centre line, its openings at its ends with openings, each lane opening moved onto its frame line (its place among
/// the arms' openings, the moved opening and the metres moved), and the place of each lane whose way does not reach
/// the line.
type Built = (Tube, Vec<Lane>, Vec<(f64, Vec<Place>)>, [Option<Opening>; 2], Vec<(usize, Opening, f64)>, Vec<[f64; 2]>);

/// Room r of `lanes`, the first its base, with its two ends (`bounds`, the seams' lines): at an end with openings,
/// the lanes' openings move in onto the frame line of the one that none lies past by more than `PAST_M`, then the
/// farthest in; at a seam the lanes end on its line; elsewhere on the line square to the base at its end. Its centre
/// line runs in the middle of its lanes' outermost edges.
fn room_of(
    r: usize,
    lanes: &[Lane],
    (bounds, seams, starts): (&[Bound; 2], &[Seam], [Option<&Start>; 2]),
    arms: &[Tube],
    arm_openings: &[Opening],
    input: &Input,
) -> Built {
    let (b, mitre) = (&input.rules.bake, obc_trailview_parts::Rules::embedded().design.mitre_max);
    let mut lanes = lanes.to_vec();
    let mut openings = arm_openings.to_vec();
    let (mut moves, mut off_line) = (Vec::new(), Vec::new());
    let base_arm = &arms[lanes[0].arm];
    // Per end its line: a point, the unit direction into the room, and per lane its metres along its arm there and
    // its point on the line.
    let mut lines: Vec<([f64; 2], [f64; 2])> = Vec::new();
    let mut fars: [Option<usize>; 2] = [None, None];
    for end in 0..2 {
        let sign = if end == 0 { 1.0 } else { -1.0 };
        match bounds[end] {
            Bound::Open => {
                let list: Vec<usize> = lanes.iter().map(|l| l.ends[end].expect("an open end")).collect();
                let way = list.iter().fold([0.0, 0.0], |w, &i| [w[0] + openings[i].dir[0], w[1] + openings[i].dir[1]]);
                let along = |i: usize| along_dir(openings[list[0]].p, openings[i].p, way);
                let past = |f: usize, i: usize| {
                    along_dir(openings[f].p, openings[i].p, openings[f].dir) > obc_trailview_parts::PAST_M
                };
                let clear: Vec<usize> = list.iter().copied().filter(|&f| !list.iter().any(|&i| past(f, i))).collect();
                let pick = if clear.is_empty() { &list } else { &clear };
                let far = pick.iter().copied().max_by(|&x, &y| along(x).total_cmp(&along(y))).unwrap();
                let (pf, df) = (openings[far].p, openings[far].dir);
                for (k, &i) in list.iter().enumerate().filter(|&(_, &i)| i != far) {
                    let arm = &arms[lanes[k].arm];
                    let Some(shift) = onto(arm, &openings[i], (pf, df), f64::INFINITY) else {
                        off_line.push(openings[i].p);
                        continue;
                    };
                    let o = openings[i].clone();
                    openings[i] = Opening { others: o.others, ..o.at(arm, shift) };
                    moves.push((i, openings[i].clone(), shift - o.shift));
                    lanes[k].span[end] = openings[i].s;
                }
                fars[end] = Some(far);
                lines.push((pf, df));
            }
            Bound::Seam(s) => {
                let seam = &seams[s];
                lines.push((seam.p, if seam.parent.0 == r { seam.dir.map(|v| -v) } else { seam.dir }));
            }
            Bound::None => {
                let d = base_arm.curve.dir(lanes[0].span[end]).map(|v| v * sign);
                lines.push((base_arm.curve.at(lanes[0].span[end]).0.p, d));
            }
        }
    }
    let ends_open = [0, 1].map(|end| bounds[end] == Bound::Open && off_line.is_empty());
    // A lane of a narrow bore (`narrow_bore_m` or less) in a room with a wider one lies at the side of the nearest
    // wider lane, edge to edge, with its way's half-width and no margin: a walk beside that tunnel, not a place of its
    // own. Narrow lanes on one side of a wider one lie side by side outward, the nearest first.
    let narrow: Vec<bool> = lanes.iter().map(|l| arms[l.arm].height <= b.narrow_bore_m).collect();
    let snug = |mut places: Vec<(f64, f64, f64)>| {
        let wide: Vec<usize> = (0..lanes.len()).filter(|&k| !narrow[k]).collect();
        if wide.is_empty() || wide.len() == lanes.len() {
            return places;
        }
        let nearest = |t: f64| {
            *wide.iter().min_by(|&&x, &&y| (places[x].0 - t).abs().total_cmp(&(places[y].0 - t).abs())).unwrap()
        };
        let mut order: Vec<(usize, usize)> =
            (0..lanes.len()).filter(|&k| narrow[k]).map(|k| (k, nearest(places[k].0))).collect();
        order.sort_by(|x, y| (places[x.0].0 - places[x.1].0).abs().total_cmp(&(places[y.0].0 - places[y.1].0).abs()));
        let mut edges: Vec<[f64; 2]> = places.iter().map(|p| [p.0 - p.1, p.0 + p.1]).collect();
        for (k, w) in order {
            let half = places[k].1 * (arms[lanes[k].arm].half - b.margin_narrow_m) / arms[lanes[k].arm].half;
            let side = if places[k].0 >= places[w].0 { 1 } else { 0 };
            let t = if side == 1 { edges[w][1] + half } else { edges[w][0] - half };
            edges[w][side] = if side == 1 { t + half } else { t - half };
            places[k] = (t, half, places[k].2);
        }
        places
    };
    // The places of the lanes on a line through p with the unit direction `left`, whose normal is `normal`: per lane its
    // metres along `left`, its half-width along it, and its metres along its arm.
    let across = |p: [f64; 2], left: [f64; 2], normal: [f64; 2], near: &[f64]| -> Option<Vec<(f64, f64, f64)>> {
        let places = (lanes.iter().zip(near))
            .map(|(l, &s)| {
                let arm = &arms[l.arm];
                let (sl, t) = crossing(arm, p, left, s)?;
                let d = arm.curve.dir(sl);
                let cos = (d[0] * normal[0] + d[1] * normal[1]).abs().max(1.0 / mitre);
                Some((t, arm.half / cos, sl))
            })
            .collect::<Option<Vec<_>>>()?;
        Some(snug(places))
    };
    let middle = |places: &[(f64, f64, f64)]| {
        let lo = places.iter().map(|p| p.0 - p.1).fold(f64::INFINITY, f64::min);
        let hi = places.iter().map(|p| p.0 + p.1).fold(f64::NEG_INFINITY, f64::max);
        (lo + hi) / 2.0
    };
    // The places on each end's line, in the room's way: across to the left of its way.
    let own_rows: Vec<EndRow> = (0..2)
        .map(|end| {
            let (p, into) = lines[end];
            let way = if end == 0 { into } else { into.map(|v| -v) };
            let left = left_of(way);
            let places: Vec<(f64, f64, f64)> = (lanes.iter())
                .map(|l| {
                    let arm = &arms[l.arm];
                    let s = l.span[end];
                    let d = arm.curve.dir(s);
                    let cos = (d[0] * way[0] + d[1] * way[1]).abs().max(1.0 / mitre);
                    (along_dir(p, arm.curve.at(s).0.p, left), arm.half / cos, s)
                })
                .collect();
            let places = snug(places);
            let c = offset(p, left, middle(&places));
            let places = places.into_iter().map(|(t, h, s)| (along_dir(c, offset(p, left, t), left), h, s)).collect();
            (c, left, places)
        })
        .collect();
    // Behind a seam the room starts at its hole, with the places of the row before the line (across to the left of the
    // heading, which is the room's way at its end 0), and its next vertex lies far enough that its first chord leaves
    // the line within `pair_max_deg`.
    let end_rows: Vec<EndRow> = (0..2)
        .map(|end| {
            let (Some(st), (_, left, own)) = (starts[end], &own_rows[end]) else { return own_rows[end].clone() };
            let sign = if end == 0 { 1.0 } else { -1.0 };
            let at = |(k, l): (usize, &Lane)| {
                st.lanes.iter().find(|x| x.0 == l.arm).map_or(own[k], |x| (sign * x.1, x.2, x.3))
            };
            (st.p, *left, lanes.iter().enumerate().map(at).collect())
        })
        .collect();
    let clear = [0, 1].map(|end| {
        let sideways = distance(end_rows[end].0, own_rows[end].0);
        (sideways / b.pair_max_deg.to_radians().tan()).max(b.vertex_gap_m)
    });
    // The centre line: on each end's line, and at each vertex of the base and the foot of each vertex of another lane
    // between them, the middle between the outermost edges of the lanes; its vertices `vertex_gap_m` apart at least.
    let [s0, s1] = lanes[0].span;
    let (lo_base, hi_base) = (s0.min(s1), s0.max(s1));
    let mut samples: Vec<f64> = base_arm.curve.stretches.iter().flat_map(|(_, ps)| ps.iter().map(|q| q.s)).collect();
    for l in lanes.iter().skip(1) {
        let (lo, hi) = (l.span[0].min(l.span[1]), l.span[0].max(l.span[1]));
        // The foot on the base of each vertex of the lane: where the base crosses the lane's normal there.
        for q in arms[l.arm].curve.stretches.iter().flat_map(|(_, ps)| ps.iter()).filter(|q| q.s > lo && q.s < hi) {
            if let Some((s, _)) = crossing(base_arm, q.p, left_of(arms[l.arm].curve.dir(q.s)), s0) {
                samples.push(s);
            }
        }
    }
    samples.retain(|&s| s > lo_base && s < hi_base);
    samples.sort_by(f64::total_cmp);
    let near_of = |s: f64| -> Vec<f64> {
        let u = (s - s0) / (s1 - s0);
        lanes.iter().map(|l| l.span[0] + (l.span[1] - l.span[0]) * u).collect()
    };
    let (start, stop) = (end_rows[0].0, end_rows[1].0);
    let mut points: Vec<[f64; 2]> = vec![start];
    for &s in &samples {
        let (q, d) = (base_arm.curve.at(s).0.p, base_arm.curve.dir(s));
        let Some(places) = across(q, left_of(d), d, &near_of(s)) else { continue };
        let c = offset(q, left_of(d), middle(&places));
        let gap = if points.len() == 1 { clear[0] } else { b.vertex_gap_m };
        if distance(points[points.len() - 1], c) >= gap && distance(c, stop) >= clear[1] {
            points.push(c);
        }
    }
    points.push(stop);
    // The section at each vertex, on the ring there: the line at an end, else the mitre of the two chords.
    let n = points.len();
    let mut table: Vec<Vec<(f64, f64, f64)>> = Vec::with_capacity(n);
    let (mut len, mut metres_at) = (0.0, Vec::with_capacity(n));
    for i in 0..n {
        if i > 0 {
            len += distance(points[i - 1], points[i]);
        }
        metres_at.push(len);
        let row = if i == 0 || i + 1 == n {
            end_rows[if i == 0 { 0 } else { 1 }].2.clone()
        } else {
            let unit = |a: [f64; 2], b: [f64; 2]| {
                let d = [b[0] - a[0], b[1] - a[1]];
                let l = d[0].hypot(d[1]);
                [d[0] / l, d[1] / l]
            };
            let (din, dout) = (unit(points[i - 1], points[i]), unit(points[i], points[i + 1]));
            let sum = [din[0] + dout[0], din[1] + dout[1]];
            let l = sum[0].hypot(sum[1]);
            let m = [sum[0] / l, sum[1] / l];
            let cos = ((m[0] * din[0] + m[1] * din[1] + m[0] * dout[0] + m[1] * dout[1]) / 2.0).max(1.0 / mitre);
            let near = table[i - 1].iter().map(|p| p.2).collect::<Vec<f64>>();
            match across(points[i], left_of(m), m, &near) {
                Some(places) => places.into_iter().map(|(t, h, s)| (t * cos, h * cos, s)).collect(),
                None => table[i - 1].clone(),
            }
        };
        table.push(row);
    }
    // The lanes from the right wall to the left, by their places at the first end.
    let mut order: Vec<usize> = (0..lanes.len()).collect();
    order.sort_by(|&x, &y| table[0][x].0.total_cmp(&table[0][y].0));
    let ordered: Vec<Lane> = order.iter().map(|&k| lanes[k].clone()).collect();
    let table: Vec<(f64, Vec<Place>)> = (metres_at.iter().zip(&table))
        .map(|(&m, row)| (m, order.iter().map(|&k| Place { off: row[k].0, half: row[k].1, s: row[k].2 }).collect()))
        .collect();
    let half = table.iter().flat_map(|(_, row)| row.iter().map(|p| p.off.abs() + p.half)).fold(0.0, f64::max);
    // The tube of the room: its centre line at the least height of its lanes, as wide as they need and as high as
    // their outlines need at the heights of the road solver.
    let design = &obc_trailview_parts::Rules::embedded().design;
    let segments = (design.arch_segments * lanes.len(), design.arch_segments);
    let mut height: f64 = 0.0;
    let mut curve_points = Vec::with_capacity(n);
    for (i, (m, row)) in table.iter().enumerate() {
        let z: Vec<f64> = (ordered.iter().zip(row)).map(|(l, p)| arms[l.arm].curve.at(p.s).0.z).collect();
        let low = z.iter().copied().fold(f64::INFINITY, f64::min);
        let outlines: Vec<(f64, f64, f64, f64)> = (ordered.iter().zip(row).zip(&z))
            .map(|((l, p), &zl)| (p.off, p.half, zl - low, arms[l.arm].height))
            .collect();
        height = height.max(hold(true, half, design.spring, &outlines, segments));
        let along = arms[ordered[0].arm].curve.at(row[0].s).0.along;
        curve_points.push(Point { p: points[i], z: low, s: *m, along });
    }
    let mut ids: Vec<u32> = lanes.iter().flat_map(|l| arms[l.arm].ids.iter().copied()).collect();
    ids.sort_unstable();
    ids.dedup();
    let tube = Tube {
        curve: Curve { stretches: vec![(base_arm.widest, curve_points)], len },
        ids,
        heights: true,
        half,
        height,
        ..base_arm.clone()
    };
    let room_opening = |end: usize| {
        let far = &openings[fars[end]?];
        let (s, p) = if end == 0 { (0.0, start) } else { (len, stop) };
        ends_open[end].then(|| Opening { shift: 0.0, s, p, floor: tube.curve.at(s).0.z, ..far.clone() })
    };
    let ends = [room_opening(0), room_opening(1)];
    (tube, ordered, table, ends, moves, off_line)
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
            for f in axes.near(p, a, t.half + widest + wall).iter().filter(|f| f.tube > a && !meet(a, f.tube)) {
                if !apart(beside(&tubes, (a, s), f), (height(a, s), height(f.tube, f.s)), input) {
                    out.entry((a, f.tube)).or_insert(p);
                }
            }
        }
    }
    out.into_values().collect()
}
