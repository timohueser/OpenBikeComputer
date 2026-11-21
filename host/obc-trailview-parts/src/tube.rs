//! The tubes: the chords of a tile by their vertices, the ring of a chord at each vertex, the inside of a chord, and
//! the light of day in a tube.

use std::collections::HashMap;

use crate::mesh::{flagged, flat, left_of, outline, Builder, Clip, Corner, Paint, Station, BORE, LAMP, PAINT};
use crate::{built, metres_per_unit, Bore, Chord, End, Part, Rules, Section};

/// An end's point lies within this of the vertex of its tube's chord.
const END_M: f64 = 0.5;
/// A lamp's half-width and its gap under the crown.
const LAMP_HALF_M: f64 = 0.15;
const LAMP_DROP_M: f64 = 0.05;

/// The key of a world point: its bits.
fn key(p: [f64; 3]) -> [u64; 2] {
    [p[0].to_bits(), p[1].to_bits()]
}

/// A chord's vertex: 0 is `a`, 1 is `b`.
fn vertex(c: &Chord, k: usize) -> [f64; 3] {
    if k == 0 {
        c.a
    } else {
        c.b
    }
}

/// The unit direction of a chord from a to b, in world units, and its metres.
pub fn direction(c: &Chord) -> ([f64; 2], f64) {
    let d = [c.b[0] - c.a[0], c.b[1] - c.a[1]];
    let l = libm::hypot(d[0], d[1]).max(f64::MIN_POSITIVE);
    ([d[0] / l, d[1] / l], l * metres_per_unit((c.a[1] + c.b[1]) / 2.0))
}

/// The chords of a tile by their vertices, and per end the chord vertex at its opening.
pub struct Net<'a> {
    /// The ends of the input, then per pair an end at the opening of its mate.
    pub ends: Vec<End>,
    pub chords: &'a [Chord],
    pub rules: &'a Rules,
    /// Per end, the other end of its pair.
    pub mate: Vec<Option<usize>>,
    at: HashMap<[u64; 2], Vec<usize>>,
    /// Per end, the chord and vertex at its opening, where its chain is built.
    opening: Vec<Option<(usize, usize)>>,
    /// The end at each opening's chord vertex.
    end_at: HashMap<(usize, usize), usize>,
    /// Per end, the rise of its floor per metre in, over its first `face_m`.
    slope: Vec<f64>,
    /// Per chord, its tube: the chords that its vertices join.
    tube: Vec<usize>,
    /// The built chords by the cells (`CELL_M`) that they come within reach of; a cell's key is its two indices' bits,
    /// so the WASM holds the code of one map of that type.
    cells: HashMap<[u64; 2], Vec<usize>>,
    /// The world units of a cell.
    cell: f64,
}

/// The side of a cell of `Net::cells`, in metres.
const CELL_M: f64 = 16.0;
/// Two tubes that meet at a cut each run this far past it: two world units, as each vertex is rounded to one.
pub const OVERLAP_M: f64 = 0.05;

impl<'a> Net<'a> {
    pub fn new(inputs: &[End], chords: &'a [Chord], rules: &'a Rules) -> Net<'a> {
        let mut at: HashMap<[u64; 2], Vec<usize>> = HashMap::new();
        for (c, chord) in chords.iter().enumerate() {
            for k in 0..2 {
                at.entry(key(vertex(chord, k))).or_default().push(c);
            }
        }
        // The chord vertex nearest a point, within `END_M`.
        let nearest = |p: [f64; 2]| {
            let m = metres_per_unit(p[1]);
            let gap = |c: usize, k: usize| {
                let v = vertex(&chords[c], k);
                libm::hypot(v[0] - p[0], v[1] - p[1]) * m
            };
            let near =
                (0..chords.len()).flat_map(|c| [(c, 0), (c, 1)]).min_by(|x, y| gap(x.0, x.1).total_cmp(&gap(y.0, y.1)));
            near.filter(|&(c, k)| gap(c, k) <= END_M)
        };
        let mut ends = inputs.to_vec();
        let mut mate = vec![None; ends.len()];
        for (e, end) in inputs.iter().enumerate() {
            let Some(across) = end.mate.filter(|_| end.part == Part::Pair) else { continue };
            let (l, m) = (left_of(end.way_in()), metres_per_unit(end.at[1]));
            let p = [end.at[0] + l[0] * across / m, end.at[1] + l[1] * across / m];
            let (Some((c, k)), Some(way), Some(shift)) = (nearest(p), end.mate_way, end.mate_shift) else { continue };
            let (chord, z) = (&chords[c], vertex(&chords[c], k)[2]);
            mate[e] = Some(ends.len());
            mate.push(Some(e));
            // Its numbers over its own floor.
            let up = |v: f64| v + end.z - z;
            ends.push(End {
                at: p,
                z,
                section: chord.section,
                half: chord.half,
                height: chord.height,
                face: up(end.face),
                left: end.left.iter().map(|&v| up(v)).collect(),
                right: end.right.iter().map(|&v| up(v)).collect(),
                foot: up(end.foot),
                mate: Some(-across),
                way,
                shift,
                mate_way: Some(end.way),
                mate_shift: Some(end.shift),
                ..end.clone()
            });
        }
        let mut opening: Vec<Option<(usize, usize)>> = (ends.iter())
            .map(|end| nearest(end.at).filter(|&(c, _)| built(&[Some(end.part)]) && built(&chords[c].ends)))
            .collect();
        // A pair is built where both of its openings are.
        for e in 0..ends.len() {
            if mate[e].is_some_and(|m| opening[m].is_none()) || (ends[e].part == Part::Pair && mate[e].is_none()) {
                opening[e] = None;
            }
        }
        let end_at = (0..ends.len()).filter_map(|e| opening[e].map(|o| (o, e))).collect();
        let n = ends.len();
        let mut net = Net {
            ends,
            chords,
            rules,
            mate,
            at,
            opening,
            end_at,
            slope: vec![0.0; n],
            tube: Vec::new(),
            cells: HashMap::new(),
            cell: 1.0,
        };
        let run = rules.design.face_m;
        net.slope = (0..n).map(|e| net.floor_at(e, run).map_or(0.0, |(s, z)| (z - net.ends[e].z) / s)).collect();
        // The tubes: each chord takes the least tube of the chords at its vertices, until none changes.
        net.tube = (0..chords.len()).collect();
        loop {
            let mut changed = false;
            for list in net.at.values() {
                let least = list.iter().map(|&c| net.tube[c]).min().unwrap_or(0);
                for &c in list {
                    if net.tube[c] != least {
                        (net.tube[c], changed) = (least, true);
                    }
                }
            }
            if !changed {
                break;
            }
        }
        // The cells of the built chords, each grown by the farthest reach of a cut.
        if let Some(first) = chords.first() {
            net.cell = CELL_M / metres_per_unit(first.a[1]);
            let shell = rules.ladder.bake.shell_m;
            let reach = 2.0 * chords.iter().map(|c| c.half + shell).fold(0.0, f64::max) / metres_per_unit(first.a[1]);
            let cell = net.cell;
            let at = |v: f64| libm::floor(v / cell) as i64;
            let key = |x: i64, y: i64| [x as u64, y as u64];
            for (c, chord) in chords.iter().enumerate().filter(|&(c, _)| built(&chords[c].ends)) {
                let lo = [chord.a[0].min(chord.b[0]) - reach, chord.a[1].min(chord.b[1]) - reach];
                let hi = [chord.a[0].max(chord.b[0]) + reach, chord.a[1].max(chord.b[1]) + reach];
                for x in at(lo[0])..=at(hi[0]) {
                    for y in at(lo[1])..=at(hi[1]) {
                        net.cells.entry(key(x, y)).or_default().push(c);
                    }
                }
            }
        }
        net
    }

    /// The metres in and the floor of end e's tube at `s` metres in along its chords, or at the end of the walk where
    /// it is shorter; None at no length.
    pub fn floor_at(&self, e: usize, s: f64) -> Option<(f64, f64)> {
        let mut last = None;
        for (c, forward, start) in self.walk(e, s) {
            let chord = &self.chords[c];
            let len = direction(chord).1;
            let (near, far) = if forward { (chord.a[2], chord.b[2]) } else { (chord.b[2], chord.a[2]) };
            if len > 0.0 {
                let at = s.min(start + len);
                last = Some((at, near + (far - near) * (at - start) / len));
            }
        }
        last.filter(|l| l.0 > 0.0)
    }

    pub fn built_end(&self, e: usize) -> bool {
        self.opening[e].is_some()
    }

    /// Whether vertex k of chord c is the opening of a built end.
    pub fn opening_at(&self, c: usize, k: usize) -> bool {
        self.end_at.contains_key(&(c, k))
    }

    /// The built end whose opening is vertex k of chord c.
    pub fn end_at(&self, c: usize, k: usize) -> Option<usize> {
        self.end_at.get(&(c, k)).copied()
    }

    pub fn built_chord(&self, c: usize) -> bool {
        built(&self.chords[c].ends)
    }

    /// The one other chord at vertex k of chord c, if exactly one shares it.
    fn other(&self, c: usize, k: usize) -> Option<usize> {
        let list = &self.at[&key(vertex(&self.chords[c], k))];
        let mut others = list.iter().copied().filter(|&o| o != c);
        let o = others.next()?;
        others.next().is_none().then_some(o)
    }

    /// The straight frame of an end: its station `s` metres in along its heading, at the grade of its first chord,
    /// across to the left of the way in.
    pub fn frame(&self, e: usize, s: f64) -> Station {
        let end = &self.ends[e];
        let (d, m) = (end.way_in(), metres_per_unit(end.at[1]));
        let p = [end.at[0] + d[0] * s / m, end.at[1] + d[1] * s / m];
        let l = left_of(d);
        Station { x: p[0], y: p[1], z: end.z + self.slope[e] * s, ax: l[0] / m, ay: l[1] / m }
    }

    /// The ring of chord c at its vertex k, across to the left of the chord: on the frame line at an opening; in the
    /// plane between the two chords where exactly one other chord shares the vertex, its width grown by the bend up to
    /// `mitre_max`; else square to the chord.
    pub fn ring(&self, c: usize, k: usize) -> Station {
        let chord = &self.chords[c];
        let (d, _) = direction(chord);
        if let Some(&e) = self.end_at.get(&(c, k)) {
            let s = self.frame(e, 0.0);
            let into = self.ends[e].way_in();
            return if d[0] * into[0] + d[1] * into[1] >= 0.0 { s } else { s.turned() };
        }
        let v = vertex(chord, k);
        let Some(o) = self.other(c, k) else { return Station::across([v[0], v[1]], v[2], d, 1.0) };
        let other = &self.chords[o];
        let (od, _) = direction(other);
        // The way through the vertex in the sense of chord c.
        let toward = if key(other.a) == key(v) { od } else { [-od[0], -od[1]] };
        let (din, dout) = if k == 1 { (d, toward) } else { ([-toward[0], -toward[1]], d) };
        let sum = [din[0] + dout[0], din[1] + dout[1]];
        let l = libm::hypot(sum[0], sum[1]);
        if l < 1e-9 {
            return Station::across([v[0], v[1]], v[2], d, 1.0);
        }
        let m = [sum[0] / l, sum[1] / l];
        let cos = (m[0] * din[0] + m[1] * din[1] + (m[0] * dout[0] + m[1] * dout[1])) / 2.0;
        Station::across([v[0], v[1]], v[2], m, 1.0 / cos.max(1.0 / self.rules.design.mitre_max))
    }

    /// The ring of chord c at vertex k across to the left of a walk that runs through c forward or back.
    pub fn ring_along(&self, c: usize, k: usize, forward: bool) -> Station {
        let s = self.ring(c, k);
        if forward {
            s
        } else {
            s.turned()
        }
    }

    /// The walk from end e into its tube: per chord, the chord, whether the walk runs from its a to its b, and the
    /// metres from the opening to the chord's start; as far as `metres`, or to a vertex that not exactly two chords
    /// share.
    pub fn walk(&self, e: usize, metres: f64) -> Vec<(usize, bool, f64)> {
        let mut out = Vec::new();
        let Some((mut c, k)) = self.opening[e] else { return out };
        let mut forward = k == 0;
        let mut s = 0.0;
        while s < metres && out.len() < self.chords.len() {
            out.push((c, forward, s));
            s += direction(&self.chords[c]).1;
            let far = if forward { 1 } else { 0 };
            let Some(o) = self.other(c, far) else { break };
            if out.iter().any(|w| w.0 == o) {
                break;
            }
            forward = key(self.chords[o].a) == key(vertex(&self.chords[c], far));
            c = o;
        }
        out
    }

    /// The share of daylight at a world point: over the openings (not the closed ends) whose section over the tube's
    /// floor, grown by the shell and a metre to each side, holds the point from the front of the head wall to
    /// `daylight_m` in, `1 − smoothstep(0, daylight_m, m)` of the metres m in from the frame line, the largest; 0 with
    /// none.
    pub fn daylight(&self, p: [f64; 3]) -> f64 {
        let (shell, reach, front) =
            (self.rules.ladder.bake.shell_m, self.rules.design.daylight_m, self.rules.design.recess_m);
        let mut most: f64 = 0.0;
        for (e, end) in self.ends.iter().enumerate().filter(|&(e, _)| !self.closed(e)) {
            let (d, m) = (end.way_in(), metres_per_unit(end.at[1]));
            let l = left_of(d);
            let (dx, dy) = (p[0] - end.at[0], p[1] - end.at[1]);
            let (along, beside) = ((dx * d[0] + dy * d[1]) * m, (dx * l[0] + dy * l[1]) * m);
            if beside.abs() > end.half + shell + 1.0 || along < -front || along > reach {
                continue;
            }
            // Over the tube's floor at that metre; in front of the frame line, over the frame's.
            let floor = if along > 0.0 {
                self.floor_at(e, along).map_or(end.z, |f| f.1)
            } else {
                end.z + self.slope[e] * along
            };
            if (end.foot..=end.height + shell).contains(&(p[2] - floor)) {
                let t = (along / reach).clamp(0.0, 1.0);
                most = most.max(1.0 - t * t * (3.0 - 2.0 * t));
            }
        }
        most
    }

    /// The inside of a chord: of its class's group.
    fn bore(&self, c: &Chord) -> Bore {
        let ladder = &self.rules.ladder;
        ladder.group(&c.class).map_or(Bore::Road, |g| ladder.groups[g].bore)
    }

    /// The height of the one other chord at vertex k of chord c, not at an opening, where it is lower than c and of its
    /// section: a step in the crown goes down to it there.
    pub fn lower(&self, c: usize, k: usize) -> Option<f64> {
        let chord = &self.chords[c];
        let other = &self.chords[self.other(c, k).filter(|_| !self.opening_at(c, k))?];
        (other.height < chord.height && other.section == chord.section).then_some(other.height)
    }

    /// The chord and vertex at end e's opening, where its chain is built.
    pub fn opening(&self, e: usize) -> Option<(usize, usize)> {
        self.opening[e]
    }

    /// Whether end e is a free end covered at its opening: its tube ends in the hill there, with no opening.
    pub fn closed(&self, e: usize) -> bool {
        self.ends[e].part == Part::Free && self.ends[e].collar <= 0.0
    }

    /// Where a station across chord c meets other built tubes whose shells overlap its own there: on each side (to the
    /// right, to the left) the nearest, at the middle between their walls on the station's line. A tube that lies over
    /// or under it by more than half their heights does not meet it.
    pub fn cuts(&self, c: usize, st: &Station) -> [Option<Cut>; 2] {
        let (own, shell) = (&self.chords[c], self.rules.ladder.bake.shell_m);
        let key = [libm::floor(st.x / self.cell) as i64 as u64, libm::floor(st.y / self.cell) as i64 as u64];
        let mut best: [Option<Cut>; 2] = [None, None];
        for &o in self.cells.get(&key).into_iter().flatten().filter(|&&o| self.tube[o] != self.tube[c]) {
            let other = &self.chords[o];
            // The station's line meets the chord: st + x·across = a + u·(b − a).
            let (dx, dy) = (other.b[0] - other.a[0], other.b[1] - other.a[1]);
            let det = dx * st.ay - st.ax * dy;
            if det.abs() < 1e-12 {
                continue;
            }
            let (px, py) = (other.a[0] - st.x, other.a[1] - st.y);
            let (x, u) = ((dx * py - dy * px) / det, (st.ax * py - st.ay * px) / det);
            // A metre past either vertex still counts: the two openings of a pair lie on their frame line within the
            // rounding of the record.
            let past = 1.0 / direction(other).1.max(1.0);
            let floor = other.a[2] + u * (other.b[2] - other.a[2]);
            if !(-past..=1.0 + past).contains(&u)
                || x.abs() > own.half + other.half + 2.0 * shell
                || (floor - st.z).abs() > (own.height + other.height) / 2.0
            {
                continue;
            }
            let side = &mut best[usize::from(x > 0.0)];
            if side.is_none_or(|b| x.abs() < b.axis.abs()) {
                let middle = (x + x.signum() * (own.half - other.half)) / 2.0;
                *side = Some(Cut { x: middle, axis: x, other: o, floor });
            }
        }
        best
    }

    /// The clips of chord c from station s to t at its cuts, on each side where it has one: the plane through them
    /// that takes off the other tube's side, and the cuts; where one station has none, the plane through the other's
    /// along the chord.
    pub fn clips(&self, c: usize, (s, t): (&Station, &Station)) -> Vec<(Clip, Option<Cut>, Option<Cut>)> {
        let (at_s, at_t) = (self.cuts(c, s), self.cuts(c, t));
        let mut out = Vec::new();
        for (cs, ct) in at_s.into_iter().zip(at_t) {
            let (st, cut) = match (cs, ct) {
                (Some(x), _) => (s, x),
                (None, Some(y)) => (t, y),
                (None, None) => continue,
            };
            let (far, near) = (st.at([cut.axis, 0.0]), st.at([cut.x, 0.0]));
            let away = [far[0] - near[0], far[1] - near[1]];
            // The two tubes overlap by `OVERLAP_M` at the plane, so the rounding of their vertices opens no gap there.
            let l = libm::hypot(away[0], away[1]).max(f64::MIN_POSITIVE);
            let d = OVERLAP_M / metres_per_unit(st.y) / l;
            let shift = |p: [f64; 3]| [p[0] + away[0] * d, p[1] + away[1] * d];
            let (a, z) = match (cs, ct) {
                (Some(x), Some(y)) => (shift(s.at([x.x, 0.0])), shift(t.at([y.x, 0.0]))),
                _ => {
                    let a = shift(near);
                    (a, [a[0] + t.x - s.x, a[1] + t.y - s.y])
                }
            };
            out.push((Clip { a, b: z, away }, cs, ct));
        }
        out
    }
}

/// The floor of a tube across: per piece from the right, its metres across and the height of its top. A road or a
/// track has a walk on each side.
pub fn floor_pieces(net: &Net, c: &Chord) -> Vec<(f64, f64, f64)> {
    let (half, road, lift, kerb) = (c.half, c.road, net.rules.design.lift_m, net.rules.design.kerb_m);
    match net.bore(c) {
        Bore::Rock => vec![(-half, half, lift)],
        Bore::Road | Bore::Track => vec![(-half, -road, kerb), (-road, road, lift), (road, half, kerb)],
    }
}

/// The spring height of a section of height `h`.
pub fn spring(net: &Net, section: Section, h: f64) -> f64 {
    if section == Section::Box {
        h
    } else {
        net.rules.design.spring * h
    }
}

/// The height where the walls of the outer shell of a section of height `h` meet its roof: the spring, or the top of a
/// box section's shell.
fn shell_spring(net: &Net, section: Section, h: f64) -> f64 {
    if section == Section::Box {
        h + net.rules.ladder.bake.shell_m
    } else {
        spring(net, section, h)
    }
}

/// The inside outline of a chord.
fn inside_outline(net: &Net, c: &Chord) -> Vec<Corner> {
    outline(c.section, c.half, c.height, spring(net, c.section, c.height), net.rules.design.arch_segments)
}

/// The outside of the shell of a section of height `h` round the inside half-width `half`, its walls down to `foot`
/// over its station.
pub fn shell_outline(net: &Net, section: Section, half: f64, h: f64, foot: f64) -> Vec<Corner> {
    let shell = net.rules.ladder.bake.shell_m;
    let mut o = outline(section, half + shell, h + shell, spring(net, section, h), net.rules.design.arch_segments);
    let last = o.len() - 1;
    (o[0][1], o[last][1]) = (foot, foot);
    o
}

/// The direction [across, up] from the middle of an outline's segment to the axis, at the spring height.
pub fn inward(a: Corner, b: Corner, spring: f64) -> Corner {
    [-(a[0] + b[0]), -(a[1] + b[1] - 2.0 * spring)]
}

/// Where a station meets another tube (`Net::cuts`): the metres across the station to the middle between their walls,
/// and the other tube there: its chord, the floor at that point (a world height) and its axis across the station.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cut {
    pub x: f64,
    pub axis: f64,
    pub other: usize,
    pub floor: f64,
}

/// The top of an outline at `x` across, the highest of its segments there; None outside it.
pub fn top_at(o: &[Corner], x: f64) -> Option<f64> {
    let mut top: Option<f64> = None;
    for w in o.windows(2) {
        let (p, q) = (w[0], w[1]);
        let (lo, hi) = (p[0].min(q[0]), p[0].max(q[0]));
        if x < lo - 1e-9 || x > hi + 1e-9 {
            continue;
        }
        let y = if hi - lo < 1e-12 { p[1].max(q[1]) } else { p[1] + (q[1] - p[1]) * (x - p[0]) / (q[0] - p[0]) };
        top = Some(top.map_or(y, |t: f64| t.max(y)));
    }
    top
}

/// The face in the plane of a cut from station s to station t: at each, from the height `lo` up to `hi` at `x` across;
/// nothing where `hi` is not over `lo`.
pub fn cut_face<L: Fn([f64; 3]) -> f64>(
    b: &mut Builder<L>,
    (s, t): (&Station, &Station),
    at_s: (f64, f64, f64),
    at_t: (f64, f64, f64),
    colour: u32,
    out: [f64; 3],
) {
    let side = |(x, lo, hi): (f64, f64, f64)| [[x, lo], [x, hi.max(lo)]];
    if at_s.2 > at_s.1 + 1e-6 || at_t.2 > at_t.1 + 1e-6 {
        b.strip((s, &side(at_s)), (t, &side(at_t)), colour, out);
    }
}

/// Where two spans at a cut differ: of the own span from `lo` to `hi`, the part under the other's span and the part
/// over it, each (lo, hi); the whole own span where the other has none.
pub fn steps((lo, hi): (f64, f64), other: Option<(f64, f64)>) -> [(f64, f64); 2] {
    match other {
        // `max` and `min`, not `clamp`: its check of the bounds would bring the formatting of floats into the WASM.
        Some((olo, ohi)) => [(lo, olo.max(lo).min(hi)), (ohi.max(lo).min(hi), hi)],
        None => [(lo, hi), (hi, hi)],
    }
}

/// The corners of a wall from its foot to its spring at `x`, with the spring of a lower section that goes on at that
/// ring between: the step face there meets the wall at it.
fn wall_side(x: f64, foot: f64, spring: f64, lower: Option<f64>) -> Vec<Corner> {
    let mut out = vec![[x, foot]];
    out.extend(lower.filter(|&l| l > foot && l < spring).map(|l| [x, l]));
    out.push([x, spring]);
    out
}

/// The inside of chord c from its ring at a to its ring at b, seen from inside: the floor by its bore, the walls from
/// the floor up and the vault, its lamps, at a vertex where a lower chord goes on the face that closes the step in the
/// crown, and at an opening the front of the floor down to the sill under it.
pub fn inside<L: Fn([f64; 3]) -> f64>(b: &mut Builder<L>, net: &Net, c: usize) {
    let chord = &net.chords[c];
    let (design, inside) = (&net.rules.design, &net.rules.inside);
    let (s, t) = (net.ring(c, 0), net.ring(c, 1));
    let (d, length) = direction(chord);
    let bore_of = net.bore(chord);
    let (half, road, lift, kerb) = (chord.half, chord.road, design.lift_m, design.kerb_m);
    let sp = spring(net, chord.section, chord.height);
    let bore = |colour: u32| flagged(colour, BORE);
    let across = |c: Corner| s.mix(&t, 0.5).out(c);
    // Of two tubes that meet, each draws its side of the middle between their walls.
    let cuts = net.clips(c, (&s, &t));
    b.clip(cuts.iter().map(|c| c.0).collect());
    // A box section is cast: its rock bore is lined.
    let cast = chord.section == Section::Box;
    let (wall, vault) = match bore_of {
        Bore::Road => (inside.tiles, inside.arch_lining),
        Bore::Track => (inside.lining, inside.lining),
        Bore::Rock if cast => (inside.lining, inside.lining),
        Bore::Rock => (inside.rock, inside.rock),
    };
    let pieces = floor_pieces(net, chord);
    if bore_of == Bore::Rock {
        b.strip(
            (&s, &[[half, lift], [-half, lift]]),
            (&t, &[[half, lift], [-half, lift]]),
            bore(if cast { inside.walk } else { inside.rock_floor }),
            across([0.0, 1.0]),
        );
    } else {
        // The road or the track: in its colour the surface code and the lines; along it, the metres of its stroke or
        // its sleepers.
        let paint = flagged(chord.code | (u32::from(chord.marked) << 8), PAINT | BORE);
        let count =
            |m: f64| if bore_of == Bore::Track { (chord.along + m) / design.sleeper_m } else { chord.along + m };
        let corner = |b: &mut Builder<L>, st: &Station, across: f64, m: f64| {
            b.vertex(st.at([across, lift]), paint, Paint { across, half: road, sleepers: count(m) })
        };
        let v = [
            corner(b, &s, -road, 0.0),
            corner(b, &s, road, 0.0),
            corner(b, &t, road, length),
            corner(b, &t, -road, length),
        ];
        b.quad(v, across([0.0, 1.0]));
        for side in [1.0, -1.0] {
            let (kerb_face, walk) =
                ([[side * road, lift], [side * road, kerb]], [[side * road, kerb], [side * half, kerb]]);
            b.strip((&s, &kerb_face), (&t, &kerb_face), bore(inside.walk), across([-side, 0.0]));
            b.strip((&s, &walk), (&t, &walk), bore(inside.walk), across([0.0, 1.0]));
        }
    }
    // The walls stand on the floor at its sides, the vault on the walls.
    let mut o = inside_outline(net, chord);
    let n = o.len();
    (o[0][1], o[n - 1][1]) = (pieces[0].2, pieces[pieces.len() - 1].2);
    let lower = |k: usize| net.lower(c, k).map(|l| spring(net, chord.section, l));
    // Each face faces the middle of the room: a box section's spring is its roof.
    let to_middle = |j: usize| across(inward(o[j], o[j + 1], chord.height / 2.0));
    for (x, foot, j) in [(-half, o[0][1], 0), (half, o[n - 1][1], n - 2)] {
        let (a, z) = (wall_side(x, foot, sp, lower(0)), wall_side(x, foot, sp, lower(1)));
        b.strip((&s, &a), (&t, &z), bore(wall), to_middle(j));
    }
    for j in 1..n - 2 {
        b.strip((&s, &o[j..j + 2]), (&t, &o[j..j + 2]), bore(vault), to_middle(j));
    }
    for k in 0..2 {
        step(b, net, c, k, &o, vault);
    }
    // At an opening (not a closed end), the front of the floor goes down to the sill, `lift_m` under the floor. Each
    // side of a piece has a corner at the top of each piece beside it, which is the corner of a face there.
    let open = |k: usize| net.end_at(c, k).is_some_and(|e| !net.closed(e));
    for (k, r) in [(0, &s), (1, &t)].into_iter().filter(|(k, _)| open(*k)) {
        let out = flat(if k == 0 { [-d[0], -d[1]] } else { d });
        for &(a0, a1, top) in &pieces {
            let side = |x: f64| {
                let mut ups: Vec<f64> =
                    (pieces.iter()).filter(|p| (p.0 == x || p.1 == x) && p.2 <= top).map(|p| p.2).collect();
                ups.sort_by(f64::total_cmp);
                ups.dedup();
                [[x, -lift]].into_iter().chain(ups.into_iter().map(|u| [x, u])).collect::<Vec<Corner>>()
            };
            b.strip((r, &side(a0)), (r, &side(a1)), bore(inside.walk), out);
        }
    }
    if bore_of == Bore::Road {
        lamps(b, net, c, (&s, &t));
    }
    b.clip(Vec::new());
    // At a cut, a face closes the room where the other tube's floor is higher or its roof lower.
    let span = |st: &Station, cut: &Cut| {
        let f = |p: &[(f64, f64, f64)], x: f64| p.iter().find(|q| x >= q.0 - 1e-9 && x <= q.1 + 1e-9).map(|q| q.2);
        let own = f(&pieces, cut.x).zip(top_at(&o, cut.x))?;
        let other = &net.chords[cut.other];
        let mut oo = inside_outline(net, other);
        let (op, m) = (floor_pieces(net, other), oo.len() - 1);
        (oo[0][1], oo[m][1]) = (op[0].2, op[op.len() - 1].2);
        let (xl, df) = (cut.x - cut.axis, cut.floor - st.z);
        Some(steps(own, f(&op, xl).zip(top_at(&oo, xl)).map(|(lo, hi)| (lo + df, hi + df))))
    };
    for (cs, ct) in cuts.iter().filter_map(|c| c.1.zip(c.2)) {
        if let (Some([ls, us]), Some([lt, ut])) = (span(&s, &cs), span(&t, &ct)) {
            let out = across([-cs.axis.signum(), 0.0]);
            cut_face(b, (&s, &t), (cs.x, ls.0, ls.1), (ct.x, lt.0, lt.1), bore(inside.walk), out);
            cut_face(b, (&s, &t), (cs.x, us.0, us.1), (ct.x, ut.0, ut.1), bore(vault), out);
        }
    }
}

/// The lamps of chord c of a road bore, from ring s to ring t: one under the crown where the metres along pass a multiple
/// of the spacing.
fn lamps<L: Fn([f64; 3]) -> f64>(b: &mut Builder<L>, net: &Net, c: usize, (s, t): (&Station, &Station)) {
    let (chord, design, inside) = (&net.chords[c], &net.rules.design, &net.rules.inside);
    let length = direction(chord).1;
    let mut at = (chord.along / design.lamp_spacing_m).ceil() * design.lamp_spacing_m;
    while at < chord.along + length {
        let m = at - chord.along;
        let (p, q) = (((m - design.lamp_m / 2.0) / length).max(0.0), ((m + design.lamp_m / 2.0) / length).min(1.0));
        let (sp, sq) = (s.mix(t, p), s.mix(t, q));
        let up = chord.height - LAMP_DROP_M;
        let light = flagged(inside.lamp, LAMP);
        let v = [
            b.at(&sp, [-LAMP_HALF_M, up], light),
            b.at(&sp, [LAMP_HALF_M, up], light),
            b.at(&sq, [LAMP_HALF_M, up], light),
            b.at(&sq, [-LAMP_HALF_M, up], light),
        ];
        b.quad(v, [0.0, 0.0, -1.0]);
        at += design.lamp_spacing_m;
    }
}

/// The face at vertex k of chord c that closes the step in the crown to a lower chord that goes on there, facing
/// into chord c.
fn step<L: Fn([f64; 3]) -> f64>(b: &mut Builder<L>, net: &Net, c: usize, k: usize, o: &[Corner], colour: u32) {
    let chord = &net.chords[c];
    let Some(h) = net.lower(c, k) else { return };
    let r = net.ring(c, k);
    let low = outline(chord.section, chord.half, h, spring(net, chord.section, h), net.rules.design.arch_segments);
    let (d, _) = direction(chord);
    let into = flat(if k == 0 { d } else { [-d[0], -d[1]] });
    for j in 1..o.len() - 2 {
        b.strip((&r, &o[j..j + 2]), (&r, &low[j..j + 2]), flagged(colour, BORE), into);
    }
}

/// The shell that goes on after the last piece of a shell: its foot (a world height), and its height and section where
/// it is lower, so that this shell closes the step.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Meets {
    pub foot: Option<f64>,
    pub lower: Option<(f64, Section)>,
}

/// A piece of a run of shell: its start and end station, its chord's height and section, and the chord.
pub type Run = (Station, Station, f64, Section, usize);

/// The outer shell of a run of pieces, with the inside half-width `half`; its walls go down to the world height `foot`.
/// Where two pieces of different height meet, a face closes the step, facing the lower one; so too after the last piece
/// where a lower shell goes on (`meets`). There the walls have a corner at the foot of the shell that goes on. Of two
/// tubes that meet, each draws its side of the middle between their walls, and a face closes the step where its shell
/// is the higher there.
pub fn shell<L: Fn([f64; 3]) -> f64>(
    b: &mut Builder<L>,
    net: &Net,
    pieces: &[Run],
    half: f64,
    foot: f64,
    colour: u32,
    meets: Meets,
) {
    let last = |i: usize| i + 1 == pieces.len();
    let next_of = |i: usize| pieces.get(i + 1).map(|p| (p.2, p.3)).or(meets.lower.filter(|_| last(i)));
    let outer = |st: &Station, h: f64, section: Section| shell_outline(net, section, half, h, foot - st.z);
    for (i, &(ref s, ref t, ref h, ref section, c)) in pieces.iter().enumerate() {
        let cuts = net.clips(c, (s, t));
        b.clip(cuts.iter().map(|c| c.0).collect());
        for (cs, ct) in cuts.iter().filter_map(|c| c.1.zip(c.2)) {
            // The other tube's shell top at the cut, over this station's floor.
            let top = |st: &Station, cut: &Cut| {
                let own = top_at(&outer(st, *h, *section), cut.x)?;
                let other = &net.chords[cut.other];
                let o = shell_outline(net, other.section, other.half, other.height, foot - cut.floor);
                Some((own, top_at(&o, cut.x - cut.axis)? + cut.floor - st.z))
            };
            if let (Some((os, xs)), Some((ot, xt))) = (top(s, &cs), top(t, &ct)) {
                let out = s.mix(t, 0.5).out([cs.axis.signum(), 0.0]);
                cut_face(b, (s, t), (cs.x, xs.min(os), os), (ct.x, xt.min(ot), ot), colour, out);
            }
        }
        let (sp, top) = (spring(net, *section, *h), shell_spring(net, *section, *h));
        let (os, ot) = (outer(s, *h, *section), outer(t, *h, *section));
        let n = os.len();
        let away = |j: usize| {
            let o = inward(os[j], os[j + 1], sp);
            s.mix(t, 0.5).out([-o[0], -o[1]])
        };
        // The spring of a lower piece that goes on before or after it: a step face meets its walls there.
        let lower = |p: Option<(f64, Section)>| {
            p.filter(|p| p.0 < *h && p.1 == *section).map(|p| shell_spring(net, *section, p.0))
        };
        let (before, after) = (lower(i.checked_sub(1).map(|k| (pieces[k].2, pieces[k].3))), lower(next_of(i)));
        let other = meets.foot.filter(|_| last(i)).map(|f| f - t.z);
        for (j, x) in [(0, os[0][0]), (n - 2, os[n - 1][0])] {
            let (a, mut z) = (wall_side(x, os[0][1], top, before), wall_side(x, ot[0][1], top, after));
            if let Some(f) = other.filter(|&f| f > z[0][1] && f < z[1][1]) {
                z.insert(1, [x, f]);
            }
            b.strip((s, &a), (t, &z), colour, away(j));
        }
        for j in 1..n - 2 {
            b.strip((s, &os[j..j + 2]), (t, &ot[j..j + 2]), colour, away(j));
        }
        let Some((next, next_section)) = next_of(i) else { continue };
        if next == *h || next_section != *section {
            continue;
        }
        // The step at t: from the taller outline to the lower, facing the lower piece.
        let (tall, low) = (outer(t, h.max(next), *section), outer(t, h.min(next), *section));
        let ahead = s.mix(t, 0.5);
        let d = [t.x - ahead.x, t.y - ahead.y];
        let toward = if next < *h { flat(d) } else { flat([-d[0], -d[1]]) };
        for j in 1..tall.len() - 2 {
            b.strip((t, &tall[j..j + 2]), (t, &low[j..j + 2]), colour, toward);
        }
    }
    b.clip(Vec::new());
}

#[cfg(test)]
pub(crate) fn outline_of(net: &Net, c: usize) -> Vec<Corner> {
    inside_outline(net, &net.chords[c])
}
