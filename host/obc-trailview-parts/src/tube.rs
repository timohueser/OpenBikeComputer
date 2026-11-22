//! The tubes: the chords of a tile by their vertices, the ring of a chord at each vertex, the inside of a chord, and
//! the light of day in a tube.

use std::collections::HashMap;

use crate::mesh::{flagged, flat, left_of, outline, Builder, Corner, Paint, Station, BORE, LAMP, PAINT};
use crate::{built, metres_per_unit, Bore, Chord, End, Lane, Mouth, Part, Rules, Section};

/// The openings of a head lie on one frame line within this: the bake leaves an opening this little past the line
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

/// A hole of a record as the builder places it: its record and its place there, its point and floor, and its numbers;
/// `foot` over its own floor.
#[derive(Clone, Debug, PartialEq)]
pub struct Opening {
    pub record: usize,
    pub across: f64,
    pub up: f64,
    pub at: [f64; 2],
    pub z: f64,
    pub part: Part,
    pub section: Section,
    pub half: f64,
    pub height: f64,
    pub collar: f64,
    pub foot: f64,
    heading: f64,
}

impl Opening {
    /// The unit direction into the tube, in world units.
    pub fn way_in(&self) -> [f64; 2] {
        [libm::cos(self.heading), libm::sin(self.heading)]
    }
}

/// The chords of a tile by their vertices, the holes of the records, and per hole the chord vertex at its opening.
pub struct Net<'a> {
    pub records: &'a [End],
    /// The holes of the records in their order.
    pub holes: Vec<Opening>,
    /// Per record, its holes.
    pub of_record: Vec<Vec<usize>>,
    pub chords: &'a [Chord],
    pub rules: &'a Rules,
    at: HashMap<[u64; 2], Vec<usize>>,
    /// Per hole, the chord and vertex at its opening, where its record and its chain are built.
    opening: Vec<Option<(usize, usize)>>,
    /// The hole at each opening's chord vertex.
    end_at: HashMap<(usize, usize), usize>,
    /// Per record, the rise of its floor per metre in, over the first `face_m` of its first hole.
    slope: Vec<f64>,
}

impl<'a> Net<'a> {
    pub fn new(records: &'a [End], chords: &'a [Chord], rules: &'a Rules) -> Net<'a> {
        let mut at: HashMap<[u64; 2], Vec<usize>> = HashMap::new();
        for (c, chord) in chords.iter().enumerate() {
            for k in 0..2 {
                at.entry(key(vertex(chord, k))).or_default().push(c);
            }
        }
        let (mut holes, mut of_record) = (Vec::new(), Vec::new());
        for (r, end) in records.iter().enumerate() {
            let (l, m) = (left_of(end.way_in()), metres_per_unit(end.at[1]));
            let mut list = Vec::new();
            for hole in &end.holes {
                list.push(holes.len());
                holes.push(Opening {
                    record: r,
                    across: hole.across,
                    up: hole.up,
                    at: [end.at[0] + l[0] * hole.across / m, end.at[1] + l[1] * hole.across / m],
                    z: end.z + hole.up,
                    part: hole.kind,
                    section: hole.section,
                    half: hole.half,
                    height: hole.height,
                    collar: hole.collar,
                    foot: end.foot - hole.up,
                    heading: end.heading,
                });
            }
            of_record.push(list);
        }
        // The chord vertex nearest a hole's point, within the error of the record's holes on one line: `PAST_M` along,
        // the rounding of two points by up to half a tile unit on each axis, and the decimals of the heading and of
        // `across` (0.025 m at a hole 40 m across) twice over.
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
        let mut opening: Vec<Option<(usize, usize)>> = (holes.iter())
            .map(|o| nearest(o.at).filter(|&(c, _)| built(&[Some(o.part)]) && built(&chords[c].ends)))
            .collect();
        // A record is built where each of its holes is.
        for list in &of_record {
            if list.iter().any(|&e| opening[e].is_none()) {
                list.iter().for_each(|&e| opening[e] = None);
            }
        }
        let end_at = (0..holes.len()).filter_map(|e| opening[e].map(|o| (o, e))).collect();
        let mut net =
            Net { records, holes, of_record, chords, rules, at, opening, end_at, slope: vec![0.0; records.len()] };
        let run = rules.design.face_m;
        net.slope = (net.of_record.iter())
            .map(|list| {
                let e = list[0];
                net.floor_at(e, run).map_or(0.0, |(s, z)| (z - net.holes[e].z) / s)
            })
            .collect();
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

    /// Whether record r is built: each of its holes is.
    pub fn built_record(&self, r: usize) -> bool {
        self.of_record[r].iter().all(|&e| self.built_end(e))
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

    /// The straight frame of record r: its station `s` metres in along its heading, at the grade of its first hole's
    /// first chord, across to the left of the way in.
    pub fn line(&self, r: usize, s: f64) -> Station {
        let end = &self.records[r];
        let (d, m) = (end.way_in(), metres_per_unit(end.at[1]));
        let p = [end.at[0] + d[0] * s / m, end.at[1] + d[1] * s / m];
        let l = left_of(d);
        Station { x: p[0], y: p[1], z: end.z + self.slope[r] * s, ax: l[0] / m, ay: l[1] / m }
    }

    /// The frame of hole e: its record's line moved to the hole's axis and floor.
    pub fn frame(&self, e: usize, s: f64) -> Station {
        let o = &self.holes[e];
        let st = self.line(o.record, s);
        Station { x: st.x + st.ax * o.across, y: st.y + st.ay * o.across, z: st.z + o.up, ..st }
    }

    /// The ring of chord c at its vertex k, across to the left of the chord: on the frame line at an opening; in the
    /// plane between the two chords where exactly one other chord shares the vertex, its width grown by the bend up to
    /// `mitre_max`; else square to the chord.
    pub fn ring(&self, c: usize, k: usize) -> Station {
        let chord = &self.chords[c];
        let (d, _) = direction(chord);
        if let Some(&e) = self.end_at.get(&(c, k)) {
            let s = self.frame(e, 0.0);
            let into = self.holes[e].way_in();
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

    /// The share of daylight at a world point: over the holes that open (not the closed ends) whose section over the
    /// tube's floor, grown by the shell and a metre to each side, holds the point from the front of the head wall to
    /// `daylight_m` in, `1 − smoothstep(0, daylight_m, m)` of the metres m in from the frame line, the largest; 0 with
    /// none.
    pub fn daylight(&self, p: [f64; 3]) -> f64 {
        let (shell, reach, front) =
            (self.rules.ladder.bake.shell_m, self.rules.design.daylight_m, self.rules.design.recess_m);
        let mut most: f64 = 0.0;
        for (e, end) in self.holes.iter().enumerate().filter(|&(e, _)| !self.closed(e)) {
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
                end.z + self.slope[end.record] * along
            };
            if (end.foot..=end.height + shell).contains(&(p[2] - floor)) {
                let t = (along / reach).clamp(0.0, 1.0);
                most = most.max(1.0 - t * t * (3.0 - 2.0 * t));
            }
        }
        most
    }

    /// The inside of a lane: of its class's group.
    fn lane_bore(&self, lane: &Lane) -> Bore {
        let ladder = &self.rules.ladder;
        ladder.group(&lane.class).map_or(Bore::Road, |g| ladder.groups[g].bore)
    }

    /// The walls and the vault of a room: of its lane with the widest carriageway, the first of those as wide.
    fn bore(&self, c: &Chord) -> Bore {
        let widest = c.lanes.iter().fold(&c.lanes[0], |w, l| if l.road > w.road { l } else { w });
        self.lane_bore(widest)
    }

    /// The height of the one other chord at vertex k of chord c, not at an opening, where it is lower than c and of its
    /// section: a step in the crown goes down to it there.
    pub fn lower(&self, c: usize, k: usize) -> Option<f64> {
        let chord = &self.chords[c];
        let other = &self.chords[self.other(c, k).filter(|_| !self.opening_at(c, k))?];
        (other.height < chord.height && other.section == chord.section).then_some(other.height)
    }

    /// The chord and vertex at hole e's opening, where its record and its chain are built.
    pub fn opening(&self, e: usize) -> Option<(usize, usize)> {
        self.opening[e]
    }

    /// Whether hole e is a free end covered at its opening: its tube ends in the hill there, with no opening.
    pub fn closed(&self, e: usize) -> bool {
        self.holes[e].part == Part::Free && self.holes[e].collar <= 0.0
    }

    /// The ways that enter record r's head: per hole from the right, per lane of its chord there from the right of
    /// the way in, the lane's point on the frame line and the metres from it to its edges there.
    pub fn mouths(&self, r: usize) -> Vec<Mouth> {
        let end = &self.records[r];
        let mut out = Vec::new();
        for &e in &self.of_record[r] {
            let Some((c, k)) = self.opening[e] else { continue };
            let (chord, st) = (&self.chords[c], self.frame(e, 0.0));
            // The chord runs against the way in where its ring is the frame turned: its right is the frame's left.
            let ring = self.ring(c, k);
            let sign = if ring.ax * st.ax + ring.ay * st.ay >= 0.0 { 1.0 } else { -1.0 };
            let mut lanes: Vec<(f64, [f64; 2])> = (0..chord.lanes.len())
                .map(|i| {
                    let (off, (lo, hi)) = (chord.lanes[i].off[k], lane_edges(chord, i, k));
                    let jamb = if sign > 0.0 { [lo - off, hi - off] } else { [off - hi, off - lo] };
                    (sign * off, jamb)
                })
                .collect();
            if sign < 0.0 {
                lanes.reverse();
            }
            for (off, jamb) in lanes {
                let p = st.at([off, 0.0]);
                out.push(([p[0], p[1]], self.holes[e].way_in(), jamb));
            }
        }
        (out.into_iter().zip(&end.ways))
            .map(|((at, into, jamb), w)| Mouth { at, into, jamb, way: w.way, shift: w.shift })
            .collect()
    }
}

/// The metres to the left of the axis of the two edges of lane i of a chord at its vertex k: the right wall or the edge
/// to the lane before, and the edge to the lane after or the left wall.
pub fn lane_edges(c: &Chord, i: usize, k: usize) -> (f64, f64) {
    let n = c.lanes.len();
    (if i == 0 { -c.half } else { c.edges[i - 1][k] }, if i + 1 == n { c.half } else { c.edges[i][k] })
}

/// The half-width of lane i of a chord at its vertex k, from edge to edge.
pub fn lane_half(c: &Chord, i: usize, k: usize) -> f64 {
    let (lo, hi) = lane_edges(c, i, k);
    (hi - lo) / 2.0
}

/// The floor of lane i of a chord at its vertex k across: per piece from the right, its metres across and the height of
/// its top over the room's floor. A road or a track lies on its carriageway, kept inside the lane's edges, with a walk
/// on each side; a walk of no width lies at the road's height, so no kerb stands where two carriageways touch.
fn lane_pieces(net: &Net, c: &Chord, i: usize, k: usize) -> Vec<(f64, f64, f64)> {
    let (lane, design) = (&c.lanes[i], &net.rules.design);
    let ((lo, hi), up) = (lane_edges(c, i, k), lane.up[k]);
    let (lift, kerb) = (up + design.lift_m, up + design.kerb_m);
    match net.lane_bore(lane) {
        Bore::Rock => vec![(lo, hi, lift)],
        Bore::Road | Bore::Track => {
            // `max` and `min`, not `clamp`: its check of the bounds would bring the formatting of floats into the WASM.
            let (r, l) = ((lane.off[k] - lane.road).max(lo).min(hi), (lane.off[k] + lane.road).min(hi).max(lo));
            let walk = |width: bool| if width { kerb } else { lift };
            vec![(lo, r, walk(r > lo)), (r, l, lift), (l, hi, walk(hi > l))]
        }
    }
}

/// The floor of a room across at vertex k of a chord: the pieces of its lanes from the right. Where two lanes meet,
/// both tops lie at their edge.
pub fn floor_pieces(net: &Net, c: &Chord, k: usize) -> Vec<(f64, f64, f64)> {
    (0..c.lanes.len()).flat_map(|i| lane_pieces(net, c, i, k)).collect()
}

/// The floor of a room at vertex k of chord c across to the left of station `st`, its ring there or that turned.
pub fn floor_along(net: &Net, c: usize, k: usize, st: &Station) -> Vec<(f64, f64, f64)> {
    let (pieces, ring) = (floor_pieces(net, &net.chords[c], k), net.ring(c, k));
    if ring.ax * st.ax + ring.ay * st.ay >= 0.0 {
        pieces
    } else {
        pieces.into_iter().rev().map(|(a, b, top)| (-b, -a, top)).collect()
    }
}

/// The segments of a chord's arch: `arch_segments` per lane, so a wide arch is as smooth as its lanes' tubes.
pub fn segments(net: &Net, c: &Chord) -> usize {
    net.rules.design.arch_segments * c.lanes.len()
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
    outline(c.section, c.half, c.height, spring(net, c.section, c.height), segments(net, c))
}

/// The outside of the shell of a section of height `h` in `segments` round the inside half-width `half`, its walls down
/// to `foot` over its station.
pub fn shell_outline(net: &Net, (section, segments): (Section, usize), half: f64, h: f64, foot: f64) -> Vec<Corner> {
    let shell = net.rules.ladder.bake.shell_m;
    let mut o = outline(section, half + shell, h + shell, spring(net, section, h), segments);
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
        let y = if hi - lo < 1e-12 {
            p[1].max(q[1])
        } else if x == p[0] || x == q[0] {
            if x == p[0] {
                p[1]
            } else {
                q[1]
            }
        } else {
            p[1] + (q[1] - p[1]) * (x - p[0]) / (q[0] - p[0])
        };
        top = Some(top.map_or(y, |t: f64| t.max(y)));
    }
    top
}

/// The corners of a wall from its foot to its spring at `x`, with the spring of a lower section that goes on at that
/// ring between: the step face there meets the wall at it.
fn wall_side(x: f64, foot: f64, spring: f64, lower: Option<f64>) -> Vec<Corner> {
    let mut out = vec![[x, foot]];
    out.extend(lower.filter(|&l| l > foot && l < spring).map(|l| [x, l]));
    out.push([x, spring]);
    out
}

/// The inside of chord c from its ring at a to its ring at b, seen from inside: per lane its floor by its bore, and a
/// riser at each edge between two lanes; the walls from the floor up and the vault, the lamps of its road lanes, at a
/// vertex where a lower chord goes on the face that closes the step in the crown, and at an opening the front of the
/// floor down to the sill under it.
pub fn inside<L: Fn([f64; 3]) -> f64>(b: &mut Builder<L>, net: &Net, c: usize) {
    let chord = &net.chords[c];
    let (design, inside) = (&net.rules.design, &net.rules.inside);
    let (s, t) = (net.ring(c, 0), net.ring(c, 1));
    let (d, length) = direction(chord);
    let (half, lift) = (chord.half, design.lift_m);
    let sp = spring(net, chord.section, chord.height);
    let bore = |colour: u32| flagged(colour, BORE);
    let across = |c: Corner| s.mix(&t, 0.5).out(c);
    // A box section is cast: its rock bore is lined.
    let cast = chord.section == Section::Box;
    let (wall, vault) = match net.bore(chord) {
        Bore::Road => (inside.tiles, inside.arch_lining),
        Bore::Track => (inside.lining, inside.lining),
        Bore::Rock if cast => (inside.lining, inside.lining),
        Bore::Rock => (inside.rock, inside.rock),
    };
    for (i, lane) in chord.lanes.iter().enumerate() {
        let [pa, pb] = [0, 1].map(|k| lane_pieces(net, chord, i, k));
        let bore_of = net.lane_bore(lane);
        if bore_of == Bore::Rock {
            let (a, z) = (pa[0], pb[0]);
            b.strip(
                (&s, &[[a.1, a.2], [a.0, a.2]]),
                (&t, &[[z.1, z.2], [z.0, z.2]]),
                bore(if cast { inside.walk } else { inside.rock_floor }),
                across([0.0, 1.0]),
            );
        } else {
            // The road or the track: in its colour the surface code and the lines; along it, the metres of its stroke
            // or its sleepers; across it, the metres from its middle.
            let paint = flagged(lane.code | (u32::from(lane.marked) << 8), PAINT | BORE);
            let count =
                |m: f64| if bore_of == Bore::Track { (lane.along + m) / design.sleeper_m } else { lane.along + m };
            let road = [pa[1].2, pb[1].2];
            let corner = |b: &mut Builder<L>, (st, k): (&Station, usize), x: f64, m: f64| {
                let paint_at = Paint { across: x - lane.off[k], half: lane.road, sleepers: count(m) };
                b.vertex(st.at([x, road[k]]), paint, paint_at)
            };
            let v = [
                corner(b, (&s, 0), pa[1].0, 0.0),
                corner(b, (&s, 0), pa[1].1, 0.0),
                corner(b, (&t, 1), pb[1].1, length),
                corner(b, (&t, 1), pb[1].0, length),
            ];
            b.quad(v, across([0.0, 1.0]));
            // Per side from the road out: the kerb's face up to the walk, and the walk to the lane's edge.
            for (side, j) in [(1.0, 2), (-1.0, 0)] {
                // The walk's edge at the road, and at the lane's edge.
                let ends = |p: (f64, f64, f64)| if side > 0.0 { (p.0, p.1) } else { (p.1, p.0) };
                let ((ra, ea), (rz, ez)) = (ends(pa[j]), ends(pb[j]));
                let (wa, wz) = (pa[j].2, pb[j].2);
                let kerb = ([[ra, pa[1].2], [ra, wa]], [[rz, pb[1].2], [rz, wz]]);
                b.strip((&s, &kerb.0), (&t, &kerb.1), bore(inside.walk), across([-side, 0.0]));
                let walk = ([[ra, wa], [ea, wa]], [[rz, wz], [ez, wz]]);
                b.strip((&s, &walk.0), (&t, &walk.1), bore(inside.walk), across([0.0, 1.0]));
            }
        }
    }
    // Where two lanes meet, a riser from the top of the one to the top of the other, facing the lower.
    for i in 1..chord.lanes.len() {
        let [(x, y), (z, w)] = [0, 1].map(|k| (lane_pieces(net, chord, i - 1, k), lane_pieces(net, chord, i, k)));
        let ([xa, ya], [xz, yz]) = ([x[x.len() - 1], y[0]], [z[z.len() - 1], w[0]]);
        let lower_left = ya.2 + yz.2 < xa.2 + xz.2;
        b.strip(
            (&s, &[[xa.1, xa.2], [ya.0, ya.2]]),
            (&t, &[[xz.1, xz.2], [yz.0, yz.2]]),
            bore(inside.walk),
            across([if lower_left { 1.0 } else { -1.0 }, 0.0]),
        );
    }
    // The walls stand on the floor at its sides, the vault on the walls.
    let [fa, fz] = [0, 1].map(|k| floor_pieces(net, chord, k));
    let o = inside_outline(net, chord);
    let n = o.len();
    let feet = [[fa[0].2, fz[0].2], [fa[fa.len() - 1].2, fz[fz.len() - 1].2]];
    let lower = |k: usize| net.lower(c, k).map(|l| spring(net, chord.section, l));
    // Each face faces the middle of the room: a box section's spring is its roof.
    let to_middle = |j: usize| across(inward(o[j], o[j + 1], chord.height / 2.0));
    for (x, foot, j) in [(-half, feet[0], 0), (half, feet[1], n - 2)] {
        let (a, z) = (wall_side(x, foot[0], sp, lower(0)), wall_side(x, foot[1], sp, lower(1)));
        b.strip((&s, &a), (&t, &z), bore(wall), to_middle(j));
    }
    for j in 1..n - 2 {
        b.strip((&s, &o[j..j + 2]), (&t, &o[j..j + 2]), bore(vault), to_middle(j));
    }
    for k in 0..2 {
        step(b, net, c, k, &o, vault);
    }
    // At an opening (not a closed end), the front of the floor goes down to the sill, `lift_m` under the room's floor.
    // Each side of a piece has a corner at the top of each piece beside it, which is the corner of a face there.
    let open = |k: usize| net.end_at(c, k).is_some_and(|e| !net.closed(e));
    for (k, r) in [(0, &s), (1, &t)].into_iter().filter(|(k, _)| open(*k)) {
        let out = flat(if k == 0 { [-d[0], -d[1]] } else { d });
        let pieces = if k == 0 { &fa } else { &fz };
        for &(a0, a1, top) in pieces {
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
    // A row of lamps under the vault over each road lane, one where the metres along its way pass a multiple of the
    // spacing.
    for lane in chord.lanes.iter().filter(|l| net.lane_bore(l) == Bore::Road) {
        let mut at = (lane.along / design.lamp_spacing_m).ceil() * design.lamp_spacing_m;
        while at < lane.along + length {
            let m = at - lane.along;
            let (p, q) = (((m - design.lamp_m / 2.0) / length).max(0.0), ((m + design.lamp_m / 2.0) / length).min(1.0));
            let (sp, sq) = (s.mix(&t, p), s.mix(&t, q));
            let off = lane.off[0] + (lane.off[1] - lane.off[0]) * (m / length).clamp(0.0, 1.0);
            // Under the crown, lower by the fall of the vault from the axis to the lane.
            let fall = top_at(&o, 0.0).zip(top_at(&o, off)).map_or(0.0, |(c, v)| c - v);
            let up = chord.height - LAMP_DROP_M - fall;
            let light = flagged(inside.lamp, LAMP);
            let v = [
                b.at(&sp, [off - LAMP_HALF_M, up], light),
                b.at(&sp, [off + LAMP_HALF_M, up], light),
                b.at(&sq, [off + LAMP_HALF_M, up], light),
                b.at(&sq, [off - LAMP_HALF_M, up], light),
            ];
            b.quad(v, [0.0, 0.0, -1.0]);
            at += design.lamp_spacing_m;
        }
    }
}

/// The face at vertex k of chord c that closes the step in the crown to a lower chord that goes on there, facing
/// into chord c.
fn step<L: Fn([f64; 3]) -> f64>(b: &mut Builder<L>, net: &Net, c: usize, k: usize, o: &[Corner], colour: u32) {
    let chord = &net.chords[c];
    let Some(h) = net.lower(c, k) else { return };
    let r = net.ring(c, k);
    let low = outline(chord.section, chord.half, h, spring(net, chord.section, h), segments(net, chord));
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

/// The outer shell of a run of pieces, each its start and end station and its chord's height and section, with the
/// inside half-width `half` and its arch in `segments`; its walls go down to the world height `foot`. Where two pieces of different height meet,
/// a face closes the step, facing the lower one; so too after the last piece where a lower shell goes on (`meets`).
/// There the walls have a corner at the foot of the shell that goes on.
pub fn shell<L: Fn([f64; 3]) -> f64>(
    b: &mut Builder<L>,
    net: &Net,
    pieces: &[(Station, Station, f64, Section)],
    (half, segments): (f64, usize),
    foot: f64,
    colour: u32,
    meets: Meets,
) {
    let last = |i: usize| i + 1 == pieces.len();
    let next_of = |i: usize| pieces.get(i + 1).map(|p| (p.2, p.3)).or(meets.lower.filter(|_| last(i)));
    let outer = |st: &Station, h: f64, section: Section| shell_outline(net, (section, segments), half, h, foot - st.z);
    for (i, (s, t, h, section)) in pieces.iter().enumerate() {
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
}

#[cfg(test)]
pub(crate) fn outline_of(net: &Net, c: usize) -> Vec<Corner> {
    inside_outline(net, &net.chords[c])
}
