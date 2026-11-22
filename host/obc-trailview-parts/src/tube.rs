//! The tubes: the chords of a tile by their vertices, the ring of a chord at each vertex, the inside of a chord, and
//! the light of day in a tube.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::mesh::{flagged, flat, left_of, outline, Builder, Clip, Corner, Paint, Station, BORE, LAMP, PAINT};
use crate::{built, metres_per_unit, Bore, Chord, End, Mate, Part, Rules, Section};

/// The two openings of a pair lie on one frame line within this: the bake leaves an opening this little past the line
/// where it lies (two openings side by side whose ways differ by a few degrees lie each a little past the other's line).
pub const PAST_M: f64 = 0.2;
/// The tile rounds a record's point to its grid: 8 world units (extent 16384 at zoom 13).
const TILE_UNIT: f64 = 8.0;
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
    /// Per chord, its tube: the chords that its vertices join; made by `meeting`.
    tube: Vec<usize>,
    /// The built chords by the cells (`CELL_M`) that they come within reach of; a cell's key is its two indices' bits,
    /// so the WASM holds the code of one map of that type. Made by `meeting`.
    cells: HashMap<[u64; 2], Vec<usize>>,
    /// The world units of a cell.
    cell: f64,
    /// Per chord, its ring at each vertex; made by `meeting`.
    rings: Vec<[Station; 2]>,
    /// Per chord, how it meets other tubes (`Met`); made as needed.
    met: RefCell<HashMap<usize, Rc<Met>>>,
    /// The cuts of `cut_on`; made as needed.
    cuts: RefCell<HashMap<CutKey, Option<Cut>>>,
}

/// A cut of `Net::cut_on`: the chord, the station (its bits), the side and the tube.
type CutKey = (usize, [u64; 4], usize, usize);

/// How a chord meets other tubes: the other tube on each side (to the right, to the left), its cut at each ring of the
/// chord, and the points where the middle between them bends: the other's cut on each of its own rings near the
/// chord.
#[derive(Debug, Default)]
struct Met {
    sides: [Option<usize>; 2],
    cuts: [[Option<Cut>; 2]; 2],
    bends: Vec<[f64; 2]>,
}

/// Where a station across chord c meets another tube (`Net::meets`), on one side: the metres across the station to
/// the middle between their walls and to the other's axis on its line, the other's chord, and the other's floor (a
/// world height) and its metres across from its axis at that middle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cut {
    pub x: f64,
    pub axis: f64,
    pub other: usize,
    pub floor: f64,
    pub across: f64,
}

/// Where a piece of a tube meets another tube on one side: the clip that takes off the other's side, its two stations
/// with their cuts, and per station whether the meeting ends there (the chord beyond that ring does not meet the
/// other).
#[derive(Clone, Copy, Debug)]
pub struct Meeting {
    pub clip: Clip,
    pub from: (Station, Cut),
    pub to: (Station, Cut),
    pub ends: [bool; 2],
}

/// How a part looks for other tubes from a station's line (`Net::meets`): the metres across to each side (to the right,
/// to the left), the metres past the planes of the other's rings, the heights over the station's floor that the
/// part's faces span (None for a room), and the one tube to find at any distance and level (None for every tube within
/// reach).
#[derive(Clone, Copy, Debug)]
pub struct Meet {
    pub across: [f64; 2],
    pub past: f64,
    pub up: Option<[f64; 2]>,
    pub only: Option<usize>,
}

/// The side of a cell of `Net::cells`, in metres.
const CELL_M: f64 = 16.0;
/// Two tubes that meet at a cut each run this far past it: two world units, as each vertex is rounded to one.
pub const OVERLAP_M: f64 = 0.05;
/// A station meets a tube this far past the planes of its rings. The two openings of a pair lie on one frame line only
/// within `PAST_M` and the rounding of their two records (the reach of `Net::new`), so near the frame line a station can
/// miss the other's first ring; the head covers that: it ends at a tube that opens within its depth (`face_m`).
pub const MEET_PAST_M: f64 = 0.1;

impl<'a> Net<'a> {
    pub fn new(inputs: &[End], chords: &'a [Chord], rules: &'a Rules) -> Net<'a> {
        let mut at: HashMap<[u64; 2], Vec<usize>> = HashMap::new();
        for (c, chord) in chords.iter().enumerate() {
            for k in 0..2 {
                at.entry(key(vertex(chord, k))).or_default().push(c);
            }
        }
        // The chord vertex nearest a point, within the error of a pair's mate placed from its record: `PAST_M` along,
        // the rounding of two points by up to half a tile unit on each axis, and the decimals of the heading and the
        // mate (0.025 m at a mate 40 m across) twice over.
        let nearest = |p: [f64; 2]| {
            let m = metres_per_unit(p[1]);
            let within = PAST_M + TILE_UNIT * std::f64::consts::SQRT_2 * m + 0.05;
            let gap = |c: usize, k: usize| {
                let v = vertex(&chords[c], k);
                libm::hypot(v[0] - p[0], v[1] - p[1]) * m
            };
            let near =
                (0..chords.len()).flat_map(|c| [(c, 0), (c, 1)]).min_by(|x, y| gap(x.0, x.1).total_cmp(&gap(y.0, y.1)));
            near.filter(|&(c, k)| gap(c, k) <= within)
        };
        let mut ends = inputs.to_vec();
        let mut mate = vec![None; ends.len()];
        for (e, end) in inputs.iter().enumerate() {
            let Some(other) = end.mate.filter(|_| end.part == Part::Pair) else { continue };
            let (l, m) = (left_of(end.way_in()), metres_per_unit(end.at[1]));
            let p = [end.at[0] + l[0] * other.across / m, end.at[1] + l[1] * other.across / m];
            let Some((c, k)) = nearest(p) else { continue };
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
                collar: other.collar,
                mate: Some(Mate { across: -other.across, way: end.way, shift: end.shift, collar: end.collar }),
                way: other.way,
                shift: other.shift,
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
            rings: Vec::new(),
            met: RefCell::new(HashMap::new()),
            cuts: RefCell::new(HashMap::new()),
        };
        let run = rules.design.face_m;
        net.slope = (0..n).map(|e| net.floor_at(e, run).map_or(0.0, |(s, z)| (z - net.ends[e].z) / s)).collect();
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
    /// `mitre_max`; else square to the chord. Kept per chord by `meeting`.
    pub fn ring(&self, c: usize, k: usize) -> Station {
        match self.rings.get(c) {
            Some(r) => r[k],
            None => self.ring_at(c, k),
        }
    }

    fn ring_at(&self, c: usize, k: usize) -> Station {
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

    /// The same net, ready for `meets`: the tube of each chord (one walk over the vertices, a union of the chords at
    /// each) and the cells of the built chords.
    pub fn meeting(mut self) -> Self {
        let n = self.chords.len();
        let mut parent: Vec<usize> = (0..n).collect();
        let root = |p: &mut Vec<usize>, mut i: usize| {
            while p[i] != i {
                p[i] = p[p[i]];
                i = p[i];
            }
            i
        };
        for list in self.at.values() {
            let r = root(&mut parent, list[0]);
            for &c in &list[1..] {
                let o = root(&mut parent, c);
                if o != r {
                    parent[o] = r;
                }
            }
        }
        self.tube = (0..n).map(|i| root(&mut parent, i)).collect();
        self.rings = (0..n).map(|c| [self.ring_at(c, 0), self.ring_at(c, 1)]).collect();
        // The cells of the built chords, each grown by the farthest reach of a tube's cut.
        let Some(first) = self.chords.first() else { return self };
        let m = metres_per_unit(first.a[1]);
        self.cell = CELL_M / m;
        let shell = self.rules.ladder.bake.shell_m;
        let reach = 2.0 * self.chords.iter().map(|c| c.half + shell).fold(0.0, f64::max) / m;
        let cell = self.cell;
        let at = |v: f64| libm::floor(v / cell) as i64;
        for (c, chord) in self.chords.iter().enumerate().filter(|&(_, ch)| built(&ch.ends)) {
            let lo = [chord.a[0].min(chord.b[0]) - reach, chord.a[1].min(chord.b[1]) - reach];
            let hi = [chord.a[0].max(chord.b[0]) + reach, chord.a[1].max(chord.b[1]) + reach];
            for x in at(lo[0])..=at(hi[0]) {
                for y in at(lo[1])..=at(hi[1]) {
                    self.cells.entry([x as u64, y as u64]).or_default().push(c);
                }
            }
        }
        self
    }

    /// Where the line of a station across chord c meets another built tube, on each side (to the right, to the left)
    /// the nearest, as `meet` looks: where the other's chord crosses the line between the planes of its rings (or past
    /// them), and the station lies within the own part's metres across and the other's half and shell of the other's
    /// axis, square to it (as the bake measures where tubes meet); where the other's room
    /// overlaps the heights that the own faces span (a head), or where its floor lies within half their heights of the
    /// station's (a room). The cut is at the middle between the two tubes' walls as the device draws them (the line
    /// through each wall's points on its chord's two rings), measured square to each, so both find one plane at any
    /// angle, also beside a bend, at a mitre or at an opening. A tube meets another for its inside and shell with the reach of its
    /// shell; a head with the reach of its wall, and so where the other crosses its frame line. Needs `meeting`.
    pub fn meets(&self, c: usize, st: &Station, meet: Meet) -> [Option<Cut>; 2] {
        debug_assert!(self.tube.len() == self.chords.len(), "meets needs Net::meeting");
        let reach = meet.across;
        let (own, shell) = (&self.chords[c], self.rules.ladder.bake.shell_m);
        let m = metres_per_unit(st.y);
        let cell = |dx: i64, dy: i64| {
            [(libm::floor(st.x / self.cell) as i64 + dx) as u64, (libm::floor(st.y / self.cell) as i64 + dy) as u64]
        };
        // The cells round the station's, where a reach passes the cell's growth.
        let wide = meet.only.is_some() || reach.iter().fold(0.0f64, |r, &x| r.max(x)) > own.half + shell + 1e-9;
        let ring = i64::from(wide);
        let mut seen: Vec<usize> = Vec::new();
        let wanted = |o: usize| self.tube[o] != self.tube[c] && meet.only.is_none_or(|t| self.tube[o] == t);
        for dx in -ring..=ring {
            for dy in -ring..=ring {
                seen.extend(self.cells.get(&cell(dx, dy)).into_iter().flatten().filter(|&&o| wanted(o)));
            }
        }
        seen.sort_unstable();
        seen.dedup();
        let unit = |d: [f64; 2]| {
            let l = libm::hypot(d[0], d[1]).max(f64::MIN_POSITIVE);
            [d[0] / l, d[1] / l]
        };
        let dot = |a: [f64; 2], b: [f64; 2]| a[0] * b[0] + a[1] * b[1];
        let (own_dir, line) = (unit([own.b[0] - own.a[0], own.b[1] - own.a[1]]), [st.ax, st.ay]);
        // The metres from the station to the wall of a chord with direction `d` on its side `out` (1 to the left), the
        // line through the wall's points on its two rings as the device draws it; and their change per unit of the
        // station's line. Square to the wall, positive past it.
        let wall = |[p, q]: [[f64; 3]; 2], d: [f64; 2], out: f64| {
            let mut n = left_of(unit([q[0] - p[0], q[1] - p[1]]));
            if dot(n, left_of(d)) * out < 0.0 {
                n = [-n[0], -n[1]];
            }
            (dot([st.x - p[0], st.y - p[1]], n) * m, dot(line, n) * m)
        };
        let mut own_wall: Option<[(f64, f64); 2]> = None;
        let mut best: [Option<Cut>; 2] = [None, None];
        for o in seen {
            let other = &self.chords[o];
            // The station's line meets the chord: st + x·across = a + u·(b − a).
            let (dx, dy) = (other.b[0] - other.a[0], other.b[1] - other.a[1]);
            let det = dx * st.ay - st.ax * dy;
            if det.abs() < 1e-12 {
                continue;
            }
            let (px, py) = (other.a[0] - st.x, other.a[1] - st.y);
            let (x, u) = ((dx * py - dy * px) / det, (st.ax * py - st.ay * px) / det);
            let side = usize::from(x > 0.0);
            let floor_at = |u: f64| other.a[2] + u.clamp(0.0, 1.0) * (other.b[2] - other.a[2]);
            let o_dir = unit([dx, dy]);
            let crossing = st.at([x, 0.0]);
            // The other's rings, each with its normal along the chord.
            let rings = [0, 1].map(|k| {
                let r = self.ring(o, k);
                let n = unit([r.ay, -r.ax]);
                (r, if dot(n, o_dir) >= 0.0 { n } else { [-n[0], -n[1]] })
            });
            let df = floor_at(u) - st.z;
            let level = match meet.up {
                Some([lo, hi]) => df < hi && df + other.height > lo,
                None => df.abs() <= (own.height + other.height) / 2.0,
            };
            // The metres from the station's line to the other's wall on the station's side, as the device draws it,
            // and to the own wall toward the other's axis: positive past the wall, and linear along the line. The
            // middle lies where the two are equal.
            let out = if dot(left_of(o_dir), [st.x - other.a[0], st.y - other.a[1]]) >= 0.0 { 1.0 } else { -1.0 };
            let (b0, kb) = wall(rings.map(|r| r.0.at([out * other.half, 0.0])), o_dir, out);
            let toward = dot(left_of(own_dir), [crossing[0] - own.a[0], crossing[1] - own.a[1]]) >= 0.0;
            let walls = *own_wall.get_or_insert_with(|| {
                let rings = [self.ring(c, 0), self.ring(c, 1)];
                [-1.0, 1.0].map(|out| wall(rings.map(|r| r.at([out * own.half, 0.0])), own_dir, out))
            });
            let (a0, ka) = walls[usize::from(toward)];
            if (ka - kb).abs() <= 1e-9 {
                continue;
            }
            let middle = (b0 - a0) / (ka - kb);
            let p = [st.x + middle * st.ax, st.y + middle * st.ay];
            // The other tube runs between the planes of its two rings at the middle: at an opening its frame line, at
            // a bend the mitre. The one tube to find is found at any distance and level: its chord's middle decided
            // the meeting.
            let between =
                |(r, n): &(Station, [f64; 2]), into: f64| into * dot([p[0] - r.x, p[1] - r.y], *n) * m >= -meet.past;
            if (meet.only.is_none() && (b0 > reach[side] + shell || !level))
                || best[side].is_some_and(|b| b.axis.abs() <= x.abs())
                || !between(&rings[0], 1.0)
                || !between(&rings[1], -1.0)
            {
                continue;
            }
            let len = libm::hypot(dx, dy).max(f64::MIN_POSITIVE);
            let along = dot([p[0] - other.a[0], p[1] - other.a[1]], o_dir) / len;
            let across = (other.half + b0 + kb * middle).abs();
            best[side] = Some(Cut { x: middle, axis: x, other: o, floor: floor_at(along), across });
        }
        best
    }

    /// How chord c meets other tubes for its inside and shell (`meets` with the reach of its shell): the nearest tube
    /// on each side that its middle meets. The bake puts a vertex where two tubes start or stop meeting, so a chord
    /// meets another all along or not at all.
    fn met(&self, c: usize) -> Rc<Met> {
        if let Some(m) = self.met.borrow().get(&c) {
            return m.clone();
        }
        let rings = [self.ring(c, 0), self.ring(c, 1)];
        let reach = self.chords[c].half + self.rules.ladder.bake.shell_m;
        let meet = Meet { across: [reach; 2], past: MEET_PAST_M, up: None, only: None };
        let sides = self.meets(c, &rings[0].mix(&rings[1], 0.5), meet).map(|cut| cut.map(|cut| self.tube[cut.other]));
        let cuts = rings.map(|r| [0, 1].map(|side| sides[side].and_then(|t| self.cut_on(c, &r, side, t))));
        // The other's cut on each of its rings beside the chord (its vertex within the reach of a meeting of the chord,
        // also past its ends, as the cut lies along the chord from the vertex's foot): there its two chords give one
        // middle.
        let mut bends: Vec<[f64; 2]> = Vec::new();
        if sides == [None, None] {
            let met = Rc::new(Met { sides, cuts, bends });
            self.met.borrow_mut().insert(c, met.clone());
            return met;
        }
        let chord = &self.chords[c];
        let (d, m) = ([chord.b[0] - chord.a[0], chord.b[1] - chord.a[1]], metres_per_unit(chord.a[1]));
        let (len2, shell) = (d[0] * d[0] + d[1] * d[1], self.rules.ladder.bake.shell_m);
        let at = |v: f64| libm::floor(v / self.cell) as i64;
        let mut seen: Vec<[u64; 2]> = Vec::new();
        for x in at(chord.a[0].min(chord.b[0]))..=at(chord.a[0].max(chord.b[0])) {
            for y in at(chord.a[1].min(chord.b[1]))..=at(chord.a[1].max(chord.b[1])) {
                for &o in self.cells.get(&[x as u64, y as u64]).into_iter().flatten() {
                    if !sides.contains(&Some(self.tube[o])) {
                        continue;
                    }
                    let other = &self.chords[o];
                    let reach = chord.half + other.half + 2.0 * shell;
                    for k in 0..2 {
                        let p = vertex(other, k);
                        let u = ((p[0] - chord.a[0]) * d[0] + (p[1] - chord.a[1]) * d[1]) / len2;
                        let foot = [chord.a[0] + u * d[0], chord.a[1] + u * d[1]];
                        let grow = reach / (len2.sqrt() * m);
                        let across = libm::hypot(p[0] - foot[0], p[1] - foot[1]) * m;
                        let beside = u > -grow && u < 1.0 + grow && across <= reach;
                        let v = key(p);
                        if !beside || seen.contains(&v) {
                            continue;
                        }
                        seen.push(v);
                        let r = self.ring(o, k);
                        let only =
                            Meet { across: [f64::INFINITY; 2], past: MEET_PAST_M, up: None, only: Some(self.tube[c]) };
                        bends.extend(self.meets(o, &r, only).iter().flatten().map(|cut| {
                            let p = r.at([cut.x, 0.0]);
                            [p[0], p[1]]
                        }));
                    }
                }
            }
        }
        let met = Rc::new(Met { sides, cuts, bends });
        self.met.borrow_mut().insert(c, met.clone());
        met
    }

    /// The cut of tube `t` on side `side` of station st across chord c, at any distance and level.
    fn cut_on(&self, c: usize, st: &Station, side: usize, t: usize) -> Option<Cut> {
        let key = (c, [st.x, st.y, st.ax, st.ay].map(f64::to_bits), side, t);
        if let Some(&cut) = self.cuts.borrow().get(&key) {
            return cut;
        }
        let meet = Meet { across: [f64::INFINITY; 2], past: MEET_PAST_M, up: None, only: Some(t) };
        let cut = self.meets(c, st, meet)[side];
        self.cuts.borrow_mut().insert(key, cut);
        cut
    }

    /// The piece of chord c from station s to t as stations, split where a station's line passes a point where the
    /// middle to a tube that the chord meets bends (`Met::bends`): the middle is one plane between two such stations.
    pub fn split(&self, c: usize, (s, t): (&Station, &Station)) -> Vec<Station> {
        let met = self.met(c);
        // The fraction of the piece whose station's line passes p: its point and its line both run linearly from s to t.
        let cross = |a: [f64; 2], b: [f64; 2]| a[0] * b[1] - a[1] * b[0];
        let (d, da) = ([t.x - s.x, t.y - s.y], [t.ax - s.ax, t.ay - s.ay]);
        let through = |p: &[f64; 2]| {
            let r = [p[0] - s.x, p[1] - s.y];
            let (q2, q1, q0) = (-cross(d, da), cross(r, da) - cross(d, [s.ax, s.ay]), cross(r, [s.ax, s.ay]));
            let first = -q0 / q1;
            if q2.abs() <= 1e-12 * q1.abs() {
                return first;
            }
            let root = (q1 * q1 - 4.0 * q2 * q0).max(0.0).sqrt();
            let pick = [(-q1 + root) / (2.0 * q2), (-q1 - root) / (2.0 * q2)];
            if (pick[0] - first).abs() <= (pick[1] - first).abs() {
                pick[0]
            } else {
                pick[1]
            }
        };
        let mut at: Vec<f64> = met.bends.iter().map(through).filter(|f| *f > 1e-6 && *f < 1.0 - 1e-6).collect();
        at.sort_by(f64::total_cmp);
        at.dedup_by(|a, b| *a - *b < 1e-9);
        [*s].into_iter().chain(at.iter().map(|&f| s.mix(t, f))).chain([*t]).collect()
    }

    /// Where the piece of chord c from station s to t meets another tube for its inside and shell: on each side whose
    /// tube the chord meets, the clip that takes off the other's side between the two stations' cuts. The meeting ends
    /// at a ring that is no opening, where no single next chord meets that tube.
    pub fn clips(&self, c: usize, (s, t): (&Station, &Station)) -> Vec<Meeting> {
        let met = self.met(c);
        if met.sides == [None, None] {
            return Vec::new();
        }
        let rings = [self.ring(c, 0), self.ring(c, 1)];
        // A piece that runs against its chord sees the chord's sides the other way round.
        let against = s.ax * rings[0].ax + s.ay * rings[0].ay < 0.0;
        let ring_of = |st: &Station| (0..2).find(|&k| libm::hypot(st.x - rings[k].x, st.y - rings[k].y) < 1e-9);
        let mut out = Vec::new();
        for side in 0..2 {
            let Some(tube) = met.sides[if against { 1 - side } else { side }] else { continue };
            let cut = |st: &Station| match ring_of(st).filter(|_| !against) {
                Some(k) => met.cuts[k][side],
                None => self.cut_on(c, st, side, tube),
            };
            let (Some(from), Some(to)) = (cut(s), cut(t)) else { continue };
            let ends = [s, t].map(|st| {
                ring_of(st).is_some_and(|k| {
                    !self.opening_at(c, k) && self.other(c, k).is_none_or(|n| !self.met(n).sides.contains(&Some(tube)))
                })
            });
            // Where the meeting ends at a ring but the other tube goes on beside the chord beyond (their levels part,
            // not their walls), the clip runs out to the own shell's wall at the ring: the room and the shell close
            // along it and are whole there, as the chord beyond is. A piece that would run out at both of its rings
            // does not meet.
            let goes_on = |st: &Station| {
                let only = Meet { across: [f64::INFINITY; 2], past: MEET_PAST_M, up: None, only: Some(tube) };
                let beyond = ring_of(st).and_then(|k| self.other(c, k));
                beyond.is_some_and(|n| self.meets(n, &self.ring(n, 0).mix(&self.ring(n, 1), 0.5), only) != [None, None])
            };
            let out_to_wall = [(s, ends[0]), (t, ends[1])].map(|(st, end)| end && goes_on(st));
            if out_to_wall == [true, true] {
                continue;
            }
            let wall = self.chords[c].half + self.rules.ladder.bake.shell_m;
            let [from, to] = [(from, out_to_wall[0]), (to, out_to_wall[1])].map(|(cut, out)| {
                if out {
                    Cut { x: cut.axis.signum() * wall, ..cut }
                } else {
                    cut
                }
            });
            // Toward the other's side of the own axis: where a narrow tube lies in a wide one's room, the middle lies
            // past the narrow one's axis.
            let away = [s.ax * from.axis.signum(), s.ay * from.axis.signum()];
            // The two tubes overlap by `OVERLAP_M` at the plane, so the rounding of their vertices opens no gap there.
            let sign = from.axis.signum();
            let shift = |st: &Station, p: [f64; 3]| {
                let w = [st.ax * sign, st.ay * sign];
                let d = OVERLAP_M / metres_per_unit(st.y) / libm::hypot(w[0], w[1]).max(f64::MIN_POSITIVE);
                [p[0] + w[0] * d, p[1] + w[1] * d]
            };
            let (a, z) = (shift(s, s.at([from.x, 0.0])), shift(t, t.at([to.x, 0.0])));
            out.push(Meeting { clip: Clip { a, b: z, away }, from: (*s, from), to: (*t, to), ends });
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
    // A box section is cast: its rock bore is lined.
    let cast = chord.section == Section::Box;
    let (wall, vault) = match bore_of {
        Bore::Road => (inside.tiles, inside.arch_lining),
        Bore::Track => (inside.lining, inside.lining),
        Bore::Rock if cast => (inside.lining, inside.lining),
        Bore::Rock => (inside.rock, inside.rock),
    };
    let pieces = floor_pieces(net, chord);
    // The road or the track: in its colour the surface code and the lines; along it, the metres of its stroke or its
    // sleepers.
    let paint = flagged(chord.code | (u32::from(chord.marked) << 8), PAINT | BORE);
    let count = |m: f64| if bore_of == Bore::Track { (chord.along + m) / design.sleeper_m } else { chord.along + m };
    // The walls stand on the floor at its sides, the vault on the walls.
    let mut o = inside_outline(net, chord);
    let n = o.len();
    (o[0][1], o[n - 1][1]) = (pieces[0].2, pieces[pieces.len() - 1].2);
    let lower = |k: usize| net.lower(c, k).map(|l| spring(net, chord.section, l));
    // Each face faces the middle of the room: a box section's spring is its roof.
    let to_middle = |j: usize| across(inward(o[j], o[j + 1], chord.height / 2.0));
    let open = |k: usize| net.end_at(c, k).is_some_and(|e| !net.closed(e));
    // Of two tubes that meet, each draws its side of the middle between their walls, piece by piece where the other
    // bends.
    let stations = net.split(c, (&s, &t));
    let metres = |st: &Station| libm::hypot(st.x - s.x, st.y - s.y) / libm::hypot(t.x - s.x, t.y - s.y) * length;
    let mut meetings = Vec::new();
    for (i, w) in stations.windows(2).enumerate() {
        let (ps, pt, first, last) = (&w[0], &w[1], i == 0, i + 2 == stations.len());
        let here = net.clips(c, (ps, pt));
        b.clip(here.iter().map(|m| m.clip).collect());
        if bore_of == Bore::Rock {
            let floor = [[half, lift], [-half, lift]];
            b.strip(
                (ps, &floor),
                (pt, &floor),
                bore(if cast { inside.walk } else { inside.rock_floor }),
                across([0.0, 1.0]),
            );
        } else {
            let corner = |b: &mut Builder<L>, st: &Station, across: f64| {
                b.vertex(st.at([across, lift]), paint, Paint { across, half: road, sleepers: count(metres(st)) })
            };
            let v = [corner(b, ps, -road), corner(b, ps, road), corner(b, pt, road), corner(b, pt, -road)];
            b.quad(v, across([0.0, 1.0]));
            for side in [1.0, -1.0] {
                let (kerb_face, walk) =
                    ([[side * road, lift], [side * road, kerb]], [[side * road, kerb], [side * half, kerb]]);
                b.strip((ps, &kerb_face), (pt, &kerb_face), bore(inside.walk), across([-side, 0.0]));
                b.strip((ps, &walk), (pt, &walk), bore(inside.walk), across([0.0, 1.0]));
            }
        }
        for (x, foot, j) in [(-half, o[0][1], 0), (half, o[n - 1][1], n - 2)] {
            let (lo, hi) = (lower(0).filter(|_| first), lower(1).filter(|_| last));
            b.strip((ps, &wall_side(x, foot, sp, lo)), (pt, &wall_side(x, foot, sp, hi)), bore(wall), to_middle(j));
        }
        for j in 1..n - 2 {
            b.strip((ps, &o[j..j + 2]), (pt, &o[j..j + 2]), bore(vault), to_middle(j));
        }
        // At a vertex the step in the crown, and at an opening (not a closed end) the front of the floor down to the
        // sill, `lift_m` under the floor. Each side of a piece has a corner at the top of each piece beside it, which
        // is the corner of a face there.
        for (k, r) in [(0, &s), (1, &t)].into_iter().filter(|&(k, _)| if k == 0 { first } else { last }) {
            step(b, net, c, k, &o, vault);
            if !open(k) {
                continue;
            }
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
            lamps(b, net, c, (&s, &t), (metres(ps), metres(pt)));
        }
        meetings.extend(here);
    }
    b.clip(Vec::new());
    // At a cut, a face closes the room where the other tube's floor is higher or its roof lower. Where the cut passes
    // the own wall within a piece, the face goes on past it at the wall's heights, in the solid between the walls.
    let span = |st: &Station, cut: &Cut| {
        let f = |p: &[(f64, f64, f64)], x: f64| p.iter().find(|q| x >= q.0 - 1e-9 && x <= q.1 + 1e-9).map(|q| q.2);
        let x = cut.x.max(pieces[0].0).min(pieces[pieces.len() - 1].1);
        let own = f(&pieces, x).zip(top_at(&o, x))?;
        let other = &net.chords[cut.other];
        let mut oo = inside_outline(net, other);
        let (op, m) = (floor_pieces(net, other), oo.len() - 1);
        (oo[0][1], oo[m][1]) = (op[0].2, op[op.len() - 1].2);
        let (xl, df) = (cut.across, cut.floor - st.z);
        Some(steps(own, f(&op, xl).zip(top_at(&oo, xl)).map(|(lo, hi)| (lo + df, hi + df))))
    };
    for m in &meetings {
        let ((s, cs), (t, ct)) = (m.from, m.to);
        if let (Some([ls, us]), Some([lt, ut])) = (span(&s, &cs), span(&t, &ct)) {
            let out = across([-cs.axis.signum(), 0.0]);
            cut_face(b, (&s, &t), (cs.x, ls.0, ls.1), (ct.x, lt.0, lt.1), bore(inside.walk), out);
            cut_face(b, (&s, &t), (cs.x, us.0, us.1), (ct.x, ut.0, ut.1), bore(vault), out);
        }
    }
}

/// The lamps of chord c of a road bore, from ring s to ring t, their parts within metres `from` to `to` along it: one
/// under the crown where the metres along the chord pass a multiple of the spacing.
fn lamps<L: Fn([f64; 3]) -> f64>(
    b: &mut Builder<L>,
    net: &Net,
    c: usize,
    (s, t): (&Station, &Station),
    (from, to): (f64, f64),
) {
    let (chord, design, inside) = (&net.chords[c], &net.rules.design, &net.rules.inside);
    let length = direction(chord).1;
    let half = design.lamp_m / 2.0;
    let first = |m: f64| (m / design.lamp_spacing_m).ceil() * design.lamp_spacing_m;
    let mut at = first(chord.along + from - half).max(first(chord.along));
    while at - half < chord.along + to && at < chord.along + length {
        let m = at - chord.along;
        let (p, q) = (((m - half).max(from) / length).max(0.0), ((m + half).min(to) / length).min(1.0));
        if q <= p {
            at += design.lamp_spacing_m;
            continue;
        }
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
    // Split where a tube that a piece meets bends, as the inside is.
    let split = |&(s, t, h, section, c): &Run| {
        net.split(c, (&s, &t)).windows(2).map(|w| (w[0], w[1], h, section, c)).collect::<Vec<Run>>()
    };
    let pieces: Vec<Run> = pieces.iter().flat_map(split).collect();
    let last = |i: usize| i + 1 == pieces.len();
    let next_of = |i: usize| pieces.get(i + 1).map(|p| (p.2, p.3)).or(meets.lower.filter(|_| last(i)));
    let outer = |st: &Station, h: f64, section: Section| shell_outline(net, section, half, h, foot - st.z);
    for (i, &(ref s, ref t, ref h, ref section, c)) in pieces.iter().enumerate() {
        let meetings = net.clips(c, (s, t));
        // Where the meeting ends at a station of the piece, a face closes the shell past the middle there, toward the
        // piece. It lies on the clip's end, so it draws unclipped.
        b.clip(Vec::new());
        for m in &meetings {
            for ((st, cut), (other, _), end) in [(m.from, m.to, m.ends[0]), (m.to, m.from, m.ends[1])] {
                if end {
                    let out = flat([other.x - st.x, other.y - st.y]);
                    end_section(b, net, c, (*h, *section, half, foot), &st, (cut.x, cut.axis.signum()), colour, out);
                }
            }
        }
        b.clip(meetings.iter().map(|m| m.clip).collect());
        for m in &meetings {
            let ((ms, cs), (mt, ct)) = (m.from, m.to);
            // The other tube's shell top at the cut, over this station's floor.
            let top = |st: &Station, cut: &Cut| {
                let own = top_at(&outer(st, *h, *section), cut.x)?;
                let other = &net.chords[cut.other];
                let o = shell_outline(net, other.section, other.half, other.height, foot - cut.floor);
                Some((own, top_at(&o, cut.across)? + cut.floor - st.z))
            };
            if let (Some((os, xs)), Some((ot, xt))) = (top(&ms, &cs), top(&mt, &ct)) {
                let out = ms.mix(&mt, 0.5).out([cs.axis.signum(), 0.0]);
                cut_face(b, (&ms, &mt), (cs.x, xs.min(os), os), (ct.x, xt.min(ot), ot), colour, out);
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

/// The face at station `st` across chord c that closes a shell where its meeting ends: the shell's section past the
/// middle `x` on the side `side` (1 to the left), between its outside and its room, from its foot up; facing `out`.
#[allow(clippy::too_many_arguments)]
fn end_section<L: Fn([f64; 3]) -> f64>(
    b: &mut Builder<L>,
    net: &Net,
    c: usize,
    (h, section, half, foot): (f64, Section, f64, f64),
    st: &Station,
    (x, side): (f64, f64),
    colour: u32,
    out: [f64; 3],
) {
    let chord = &net.chords[c];
    // In the frame mirrored to the side, so the part past the middle runs to rising metres across.
    let mirror = |o: Vec<Corner>| {
        let mut m: Vec<Corner> = o.into_iter().map(|p| [side * p[0], p[1]]).collect();
        if side < 0.0 {
            m.reverse();
        }
        m
    };
    let xm = side * x;
    let base = foot - st.z;
    let shell = mirror(shell_outline(net, section, half, h, base));
    let Some(top) = top_at(&shell, xm) else { return };
    let past = |o: &[Corner]| o.iter().copied().filter(|p| p[0] > xm + 1e-9).collect::<Vec<Corner>>();
    let mut outside = vec![[xm, top]];
    outside.extend(past(&shell));
    outside.push([xm, base]);
    let pieces = floor_pieces(net, chord);
    let mut room = inside_outline(net, chord);
    let n = room.len();
    (room[0][1], room[n - 1][1]) = (pieces[0].2, pieces[pieces.len() - 1].2);
    let room = mirror(room);
    let mut floor: Vec<Corner> = pieces.iter().flat_map(|p| [[p.0, p.2], [p.1, p.2]]).collect();
    floor.dedup();
    let floor = mirror(floor);
    let inner = match (top_at(&room, xm), floor.iter().rev().find(|p| p[0] <= xm + 1e-9)) {
        (Some(roof), Some(&[_, level])) => {
            let mut i = vec![[xm, roof]];
            i.extend(past(&room));
            i.extend(past(&floor).into_iter().rev());
            i.push([xm, level]);
            i.dedup();
            i
        }
        _ => vec![[xm, top], [xm, base]],
    };
    let back = |o: Vec<Corner>| o.into_iter().map(|p| [side * p[0], p[1]]).collect::<Vec<Corner>>();
    b.strip((st, &back(outside)), (st, &back(inner)), colour, out);
}

#[cfg(test)]
pub(crate) fn outline_of(net: &Net, c: usize) -> Vec<Corner> {
    inside_outline(net, &net.chords[c])
}
