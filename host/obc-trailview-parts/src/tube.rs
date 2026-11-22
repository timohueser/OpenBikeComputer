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
    /// Per chord and vertex the built hole whose opening it is.
    end_at: Vec<[Option<usize>; 2]>,
    /// Per record, the rise of its floor per metre in, over the first `face_m` of its first hole.
    slope: Vec<f64>,
}

impl<'a> Net<'a> {
    pub fn new(records: &'a [End], chords: &'a [Chord], rules: &'a Rules) -> Net<'a> {
        // The chords at each vertex. A chord that does not draw (a tunnel way that ends on this one's side, as a cross
        // passage does) does not bend the rings of those that do.
        let mut at: HashMap<[u64; 2], Vec<usize>> = HashMap::new();
        for (c, chord) in chords.iter().enumerate().filter(|(_, chord)| built(&chord.ends)) {
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
        // A seam's hole is a chord whose end there is the seam, on the hole's side of the line: the first behind the
        // heading, the others in front of it; so a chord that ends at the seam's point and one that starts there are
        // told apart.
        let nearest = |o: &Opening, first: bool| {
            let (p, m) = (o.at, metres_per_unit(o.at[1]));
            let within = PAST_M + TILE_UNIT * std::f64::consts::SQRT_2 * m + 0.05;
            let gap = |c: usize, k: usize| {
                let v = vertex(&chords[c], k);
                libm::hypot(v[0] - p[0], v[1] - p[1]) * m
            };
            let side = |c: usize, k: usize| {
                if o.part != Part::Seam {
                    return true;
                }
                let (d, into) = (direction(&chords[c]).0, o.way_in());
                let away = if k == 0 { d } else { [-d[0], -d[1]] };
                let ahead = away[0] * into[0] + away[1] * into[1] > 0.0;
                chords[c].ends[k] == Some(Part::Seam) && ahead != first
            };
            let near = (0..chords.len())
                .flat_map(|c| [(c, 0), (c, 1)])
                .filter(|&(c, k)| side(c, k))
                .min_by(|x, y| gap(x.0, x.1).total_cmp(&gap(y.0, y.1)));
            near.filter(|&(c, k)| gap(c, k) <= within)
        };
        let mut opening: Vec<Option<(usize, usize)>> = (holes.iter().enumerate())
            .map(|(e, o)| {
                let first = of_record[o.record][0] == e;
                nearest(o, first).filter(|&(c, _)| built(&[Some(o.part)]) && built(&chords[c].ends))
            })
            .collect();
        // A record is built where each of its holes is.
        for list in &of_record {
            if list.iter().any(|&e| opening[e].is_none()) {
                list.iter().for_each(|&e| opening[e] = None);
            }
        }
        let mut end_at = vec![[None; 2]; chords.len()];
        for (e, o) in opening.iter().enumerate() {
            if let Some((c, k)) = *o {
                end_at[c][k] = Some(e);
            }
        }
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
        self.end_at[c][k].is_some()
    }

    /// The built end whose opening is vertex k of chord c.
    pub fn end_at(&self, c: usize, k: usize) -> Option<usize> {
        self.end_at[c][k]
    }

    pub fn built_chord(&self, c: usize) -> bool {
        built(&self.chords[c].ends)
    }

    /// Whether another chord of a built chain shares vertex k of chord c.
    pub fn meets(&self, c: usize, k: usize) -> bool {
        self.at.get(&key(vertex(&self.chords[c], k))).is_some_and(|list| list.iter().any(|&o| o != c))
    }

    /// The one other chord at vertex k of chord c, if exactly one shares it.
    fn other(&self, c: usize, k: usize) -> Option<usize> {
        let list = self.at.get(&key(vertex(&self.chords[c], k)))?;
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
        if let Some(e) = self.end_at[c][k] {
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
            if self.end_at[c][far].is_some() {
                break;
            }
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
        for (e, end) in self.holes.iter().enumerate().filter(|&(e, o)| !self.closed(e) && o.part != Part::Seam) {
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
    pub fn bore(&self, c: &Chord) -> Bore {
        let widest = c.lanes.iter().fold(&c.lanes[0], |w, l| if l.road > w.road { l } else { w });
        self.lane_bore(widest)
    }

    /// The height at vertex k of chord c of the one other chord there, not at an opening, where it is lower than c's
    /// and of its section: a step in the crown goes down to it there.
    pub fn lower(&self, c: usize, k: usize) -> Option<f64> {
        let chord = &self.chords[c];
        let o = self.other(c, k).filter(|_| !self.opening_at(c, k))?;
        let other = &self.chords[o];
        let h = other.height[if key(other.a) == key(vertex(chord, k)) { 0 } else { 1 }];
        (h < chord.height[k] && other.section == chord.section).then_some(h)
    }

    /// The chord and vertex at hole e's opening, where its record and its chain are built.
    pub fn opening(&self, e: usize) -> Option<(usize, usize)> {
        self.opening[e]
    }

    /// Whether hole e is a free end covered at its opening: its tube ends in the hill there, with no opening.
    pub fn closed(&self, e: usize) -> bool {
        self.holes[e].part == Part::Free && self.holes[e].collar <= 0.0
    }

    /// The seam record whose hole is vertex k of chord c, and the place of that hole in it (0: the room before the
    /// line).
    pub fn seam_at(&self, c: usize, k: usize) -> Option<(usize, usize)> {
        let e = self.end_at(c, k).filter(|&e| self.holes[e].part == Part::Seam)?;
        let r = self.holes[e].record;
        Some((r, self.of_record[r].iter().position(|&h| h == e)?))
    }

    /// The inside half-width of chord c at its vertex k: at a room behind a seam the hole's, along the seam's line.
    pub fn half_at(&self, c: usize, k: usize) -> f64 {
        match self.seam_at(c, k) {
            Some((r, h)) if h > 0 => self.holes[self.of_record[r][h]].half,
            _ => self.chords[c].half,
        }
    }

    /// The metres to the left of the axis of the two edges of lane i of chord c at its vertex k: the right wall or the
    /// edge to the lane before, and the edge to the lane after or the left wall.
    pub fn lane_edges(&self, c: usize, i: usize, k: usize) -> (f64, f64) {
        let (chord, half) = (&self.chords[c], self.half_at(c, k));
        let n = chord.lanes.len();
        (if i == 0 { -half } else { chord.edges[i - 1][k] }, if i + 1 == n { half } else { chord.edges[i][k] })
    }

    /// Seam record r on its line, in the metres across to the left of its line and up from its floor, each corner with
    /// its world point: the floor of the room before the line from its right wall to its left, both tops at a riser
    /// and a corner where a hole's side stands on it; that room's outline from its right spring to its left, with a
    /// corner over each hole's side; and per room behind the line its outline from its right floor corner up and over to
    /// its left floor corner, the floor corners those of the room before. Each corner is the corner of the ring that
    /// draws it.
    pub fn seam_cut(&self, r: usize) -> Option<SeamCut> {
        let list = &self.of_record[r];
        let line = self.line(r, 0.0);
        // A ring's corners in the line's metres: the ring runs with the line or against it.
        let sign = |st: &Station| if st.ax * line.ax + st.ay * line.ay >= 0.0 { 1.0 } else { -1.0 };
        let (cp, kp) = self.opening(list[0])?;
        let (chord, ring) = (&self.chords[cp], self.ring(cp, kp));
        let sp = sign(&ring);
        // The sides of the holes behind the line, in the parent's metres.
        let mut sides: Vec<f64> = Vec::new();
        let mut outlines = Vec::new();
        for &e in &list[1..] {
            let (c, k) = self.opening(e)?;
            let child = &self.chords[c];
            let (half, st) = (self.half_at(c, k), self.ring(c, k));
            let pieces = floor_pieces(self, c, k);
            let o = inside_outline(self, child, k, half);
            let mut corners = vec![[-half, pieces[0].2]];
            corners.extend_from_slice(&o[1..o.len() - 1]);
            corners.push([half, pieces[pieces.len() - 1].2]);
            let (sc, h) = (sign(&st), &self.holes[e]);
            // In the line's metres from the right; the floor corners are the parent's.
            let mut on: Vec<(Corner, [f64; 3])> =
                corners.iter().map(|&q| ([h.across + sc * q[0], h.up + q[1]], st.at(q))).collect();
            if sc < 0.0 {
                on.reverse();
            }
            sides.extend([on[0].0[0], on[on.len() - 1].0[0]].map(|x| sp * x));
            outlines.push(on);
        }
        // Per door its outline from the floor at its right side up to its top and down its left side, its top under the
        // spring of the room before the line.
        let sp_parent = spring(self, chord.section, chord.height[kp]);
        for door in &self.records[r].doors {
            let (lo, hi, top) =
                (door.across - door.half, door.across + door.half, (door.up + door.height).min(sp_parent));
            let on: Vec<(Corner, [f64; 3])> =
                [[lo, 0.0], [lo, top], [hi, top], [hi, 0.0]].map(|q| (q, ring.at([sp * q[0], q[1]]))).to_vec();
            sides.extend([lo, hi].map(|x| sp * x));
            outlines.push(on);
        }
        // The parent's floor, with a corner where a hole's side stands on it.
        let mut floor: Vec<Corner> = Vec::new();
        for (x0, x1, top) in floor_pieces(self, cp, kp) {
            floor.push([x0, top]);
            floor.extend(sides.iter().filter(|&&x| x > x0 && x < x1).map(|&x| [x, top]));
            floor.push([x1, top]);
        }
        floor.sort_by(|a, b| a[0].total_cmp(&b[0]));
        floor.dedup();
        // The parent's outline from spring to spring, with a corner over each hole's side, as high as the hole there.
        let o = inside_outline(self, chord, kp, self.half_at(cp, kp));
        let mut top: Vec<Corner> = o[1..o.len() - 1].to_vec();
        for (i, &x) in sides.iter().enumerate() {
            let hole = &outlines[i / 2];
            let spring_at = hole.iter().find(|q| (sp * q.0[0] - x).abs() < 1e-9).map(|q| q.0[1]).unwrap_or(0.0);
            let below =
                hole.iter().filter(|q| (sp * q.0[0] - x).abs() < 1e-9).map(|q| q.0[1]).fold(spring_at, f64::max);
            top.push([x, top_at(&o, x).unwrap_or(0.0).max(below)]);
        }
        top.sort_by(|a, b| a[0].total_cmp(&b[0]));
        top.dedup();
        let in_line = |list: Vec<Corner>| {
            let mut out: Vec<(Corner, [f64; 3])> = list.into_iter().map(|q| ([sp * q[0], q[1]], ring.at(q))).collect();
            if sp < 0.0 {
                out.reverse();
            }
            out
        };
        let (floor, top) = (in_line(floor), in_line(top));
        // A hole's floor corners are the parent's corners at its sides.
        for hole in &mut outlines {
            let n = hole.len();
            for k in [0, n - 1] {
                if let Some(f) = floor.iter().find(|f| (f.0[0] - hole[k].0[0]).abs() < 1e-3) {
                    hole[k] = *f;
                }
            }
        }
        let doors = (0..outlines.len()).map(|h| h + 1 >= list.len()).collect();
        Some(SeamCut { half: self.half_at(cp, kp), floor, top, holes: outlines, doors })
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
                    let (off, (lo, hi)) = (chord.lanes[i].off[k], self.lane_edges(c, i, k));
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

/// A seam on its line (`Net::seam_cut`): per corner its metres across the line and up from the record's floor, and its
/// world point.
#[derive(Clone, Debug)]
pub struct SeamCut {
    pub half: f64,
    pub floor: Vec<(Corner, [f64; 3])>,
    pub top: Vec<(Corner, [f64; 3])>,
    pub holes: Vec<Vec<(Corner, [f64; 3])>>,
    /// Per hole whether it is a door, which a dark face closes.
    pub doors: Vec<bool>,
}

/// The floor of lane i of a chord at its vertex k across: per piece from the right, its metres across and the height of
/// its top over the room's floor. Behind a seam's hole it is the floor of the room before the line there (`behind`),
/// so each corner on the line has one source. Else a road or a track lies on its carriageway, kept inside the lane's
/// edges, with a walk on each side; a walk of no width lies at the road's height, so no kerb stands where two
/// carriageways touch.
fn lane_pieces(net: &Net, c: usize, i: usize, k: usize) -> Vec<(f64, f64, f64)> {
    let seam = net.seam_at(c, k).filter(|&(_, h)| h > 0);
    seam.and_then(|(r, h)| behind(net, (r, h), (c, k), i)).unwrap_or_else(|| own_pieces(net, c, i, k))
}

/// The floor of lane i of chord c at its vertex k in hole h of seam record r: of the lanes of the room before the line
/// that lie in the hole, the one in its place, cut to the hole, in the chord's metres and over its floor; a piece that
/// the cut leaves with no width lies at the height of the next piece in the hole. None where the hole holds another
/// count of lanes.
fn behind(net: &Net, (r, h): (usize, usize), (c, k): (usize, usize), i: usize) -> Option<Vec<(f64, f64, f64)>> {
    let (cp, kp) = net.opening(net.of_record[r][0])?;
    let hole = &net.holes[net.of_record[r][h]];
    let line = net.line(r, 0.0);
    let sign = |st: Station| if st.ax * line.ax + st.ay * line.ay >= 0.0 { 1.0 } else { -1.0 };
    let (sp, sc) = (sign(net.ring(cp, kp)), sign(net.ring(c, k)));
    // A run of pieces from the right of one metres to the right of the other, by the sign between them.
    let turn = |pieces: Vec<(f64, f64, f64)>, sign: f64, at: f64| -> Vec<(f64, f64, f64)> {
        let moved =
            pieces.into_iter().map(|(a, b, t)| if sign > 0.0 { (a - at, b - at, t) } else { (at - b, at - a, t) });
        let mut out: Vec<_> = moved.collect();
        if sign < 0.0 {
            out.reverse();
        }
        out
    };
    let (lo, hi) = (hole.across - hole.half, hole.across + hole.half);
    // The lanes of the room before the line in the line's metres from the right, those in the hole.
    let mut lanes: Vec<Vec<(f64, f64, f64)>> =
        (0..net.chords[cp].lanes.len()).map(|j| turn(own_pieces(net, cp, j, kp), sp, 0.0)).collect();
    if sp < 0.0 {
        lanes.reverse();
    }
    lanes.retain(|l| l[0].0 < hi && l[l.len() - 1].1 > lo);
    let n = net.chords[c].lanes.len();
    if lanes.len() != n {
        return None;
    }
    let lane = &lanes[if sc > 0.0 { i } else { n - 1 - i }];
    let mut cut: Vec<(f64, f64, f64)> =
        lane.iter().map(|&(a, b, t)| (a.max(lo).min(hi), b.min(hi).max(lo), t - hole.up)).collect();
    for j in 0..cut.len() {
        if cut[j].0 == cut[j].1 {
            let next = (j..cut.len()).chain((0..j).rev()).find(|&q| cut[q].0 < cut[q].1);
            cut[j].2 = next.map_or(cut[j].2, |q| cut[q].2);
        }
    }
    Some(turn(cut, sc, hole.across))
}

/// The floor of lane i of chord c at its vertex k by its own numbers (`lane_pieces`).
fn own_pieces(net: &Net, c: usize, i: usize, k: usize) -> Vec<(f64, f64, f64)> {
    let (lane, design) = (&net.chords[c].lanes[i], &net.rules.design);
    let ((lo, hi), up) = (net.lane_edges(c, i, k), lane.up[k]);
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
pub fn floor_pieces(net: &Net, c: usize, k: usize) -> Vec<(f64, f64, f64)> {
    (0..net.chords[c].lanes.len()).flat_map(|i| lane_pieces(net, c, i, k)).collect()
}

/// The floor of a room at vertex k of chord c across to the left of station `st`, its ring there or that turned.
pub fn floor_along(net: &Net, c: usize, k: usize, st: &Station) -> Vec<(f64, f64, f64)> {
    let (pieces, ring) = (floor_pieces(net, c, k), net.ring(c, k));
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

/// The inside outline of a chord at its vertex k, `half` wide.
fn inside_outline(net: &Net, c: &Chord, k: usize, half: f64) -> Vec<Corner> {
    outline(c.section, half, c.height[k], spring(net, c.section, c.height[k]), segments(net, c))
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

/// A room behind a seam: the floor corners of the room before, by the line's metres, the room's place across and up,
/// and its way across.
type Behind<'a> = (&'a [(Corner, [f64; 3])], [f64; 2], f64);

/// The corners of a ring of chord c at its vertex k: by default the ring's own; on a seam's line the corners that the
/// seam shares, each from the ring that draws it.
struct Corners<'a> {
    ring: Station,
    /// The room before the seam: its floor and its outline from spring to spring, with the seam's corners, and the
    /// corners of a hole's side on each wall; in the chord's metres.
    parent: Option<(Vec<Corner>, Vec<Corner>, Vec<Corner>)>,
    /// A room behind the seam: the floor corners of the room before, by the line's metres, and how this ring's metres
    /// map onto them: its place across and up, and its way across.
    child: Option<Behind<'a>>,
}

impl Corners<'_> {
    fn new<'a>(net: &Net, c: usize, k: usize, cut: Option<&'a SeamCut>) -> Corners<'a> {
        let ring = net.ring(c, k);
        let (Some((r, h)), Some(cut)) = (net.seam_at(c, k), cut) else {
            return Corners { ring, parent: None, child: None };
        };
        let line = net.line(r, 0.0);
        let sign = if ring.ax * line.ax + ring.ay * line.ay >= 0.0 { 1.0 } else { -1.0 };
        let mine = |list: &[(Corner, [f64; 3])]| {
            let mut out: Vec<Corner> = list.iter().map(|q| [sign * q.0[0], q.0[1]]).collect();
            out.sort_by(|a, b| a[0].total_cmp(&b[0]));
            out
        };
        if h == 0 {
            let walls =
                mine(&cut.holes.iter().flatten().copied().filter(|q| q.0[0].abs() >= cut.half).collect::<Vec<_>>());
            return Corners { ring, parent: Some((mine(&cut.floor), mine(&cut.top), walls)), child: None };
        }
        let hole = &net.holes[net.of_record[r][h]];
        Corners { ring, parent: None, child: Some((&cut.floor, [hole.across, hole.up], sign)) }
    }

    /// The point of a corner of the floor or of a wall's foot.
    fn floor(&self, q: Corner) -> [f64; 3] {
        match self.child {
            Some((floor, [across, up], sign)) => {
                let (x, y) = (across + sign * q[0], up + q[1]);
                let gap = |f: &&(Corner, [f64; 3])| (f.0[0] - x).abs().max((f.0[1] - y).abs());
                let near = floor.iter().min_by(|a, b| gap(a).total_cmp(&gap(b)));
                near.filter(|f| gap(f) < 1e-3).map_or(self.ring.at(q), |f| f.1)
            }
            None => self.ring.at(q),
        }
    }

    /// The corners of a run of the floor from `a` to `b` at one height: on the seam's line, with the seam's corners
    /// between.
    fn floor_run(&self, a: Corner, b: Corner) -> Vec<Corner> {
        let (lo, hi) = (a[0].min(b[0]), a[0].max(b[0]));
        let mut out = vec![a];
        if let Some((floor, ..)) = &self.parent {
            let between = floor.iter().filter(|q| q[0] > lo && q[0] < hi && q[1] == a[1]);
            let mut inner: Vec<Corner> = between.copied().collect();
            if a[0] > b[0] {
                inner.reverse();
            }
            out.extend(inner);
        }
        out.push(b);
        out
    }

    /// The corners of a wall at `x` from `a` up to `b`: on the seam's line, with the corners of a hole's side on it.
    fn wall_run(&self, a: Corner, b: Corner) -> Vec<Corner> {
        let mut out = vec![a];
        if let Some((_, _, walls)) = &self.parent {
            let mut on: Vec<Corner> =
                walls.iter().filter(|q| q[0] == a[0] && q[1] > a[1] && q[1] < b[1]).copied().collect();
            on.sort_by(|p, q| p[1].total_cmp(&q[1]));
            out.extend(on);
        }
        out.push(b);
        out
    }

    /// The corners of a run of the outline from `a` to `b`: on the seam's line, with the seam's corners between.
    fn top_run(&self, a: Corner, b: Corner) -> Vec<Corner> {
        let mut out = vec![a];
        if let Some((_, top, _)) = &self.parent {
            let (lo, hi) = (a[0].min(b[0]), a[0].max(b[0]));
            out.extend(top.iter().filter(|q| q[0] > lo && q[0] < hi).copied());
        }
        out.push(b);
        out
    }
}

/// The inside of chord c from its ring at a to its ring at b, seen from inside: per lane its floor by its bore, and a
/// riser at each edge between two lanes; the walls from the floor up and the vault, the lamps of its road lanes, at a
/// vertex where a lower chord goes on the face that closes the step in the crown, and at an opening the front of the
/// floor down to the sill under it.
pub fn inside<L: Fn([f64; 3]) -> f64>(b: &mut Builder<L>, net: &Net, c: usize) {
    let chord = &net.chords[c];
    let (design, inside) = (&net.rules.design, &net.rules.inside);
    let cuts = [0, 1].map(|k| net.seam_at(c, k).and_then(|(r, _)| net.seam_cut(r)));
    let rings = [0, 1].map(|k| Corners::new(net, c, k, cuts[k].as_ref()));
    let (s, t) = (rings[0].ring, rings[1].ring);
    let (d, length) = direction(chord);
    let lift = design.lift_m;
    let sp = chord.height.map(|h| spring(net, chord.section, h));
    let bore = |colour: u32| flagged(colour, BORE);
    let across = |c: Corner| s.mix(&t, 0.5).out(c);
    // A strip between corners of the floor at a and at b.
    let floor_strip = |b: &mut Builder<L>, (qa, qz): (&[Corner], &[Corner]), colour: u32, out: [f64; 3]| {
        let (pa, pz): (Vec<[f64; 3]>, Vec<[f64; 3]>) =
            (qa.iter().map(|&q| rings[0].floor(q)).collect(), qz.iter().map(|&q| rings[1].floor(q)).collect());
        b.strip_at((&pa, qa), (&pz, qz), colour, out);
    };
    // A box section is cast: its rock bore is lined.
    let cast = chord.section == Section::Box;
    let (wall, vault) = match net.bore(chord) {
        Bore::Road => (inside.tiles, inside.arch_lining),
        Bore::Track => (inside.lining, inside.lining),
        Bore::Rock if cast => (inside.lining, inside.lining),
        Bore::Rock => (inside.rock, inside.rock),
    };
    for (i, lane) in chord.lanes.iter().enumerate() {
        let [pa, pb] = [0, 1].map(|k| lane_pieces(net, c, i, k));
        let bore_of = net.lane_bore(lane);
        if bore_of == Bore::Rock {
            let (a, z) = (pa[0], pb[0]);
            let runs = [rings[0].floor_run([a.1, a.2], [a.0, a.2]), rings[1].floor_run([z.1, z.2], [z.0, z.2])];
            let colour = bore(if cast { inside.walk } else { inside.rock_floor });
            floor_strip(b, (&runs[0], &runs[1]), colour, across([0.0, 1.0]));
        } else {
            // The road or the track: in its colour the surface code and the lines; along it, the metres of its stroke
            // or its sleepers; across it, the metres from its middle.
            let paint = flagged(lane.code | (u32::from(lane.marked) << 8), PAINT | BORE);
            let count =
                |m: f64| if bore_of == Bore::Track { (lane.along + m) / design.sleeper_m } else { lane.along + m };
            let road = [pa[1].2, pb[1].2];
            let corner = |b: &mut Builder<L>, k: usize, x: f64, m: f64| {
                let paint_at = Paint { across: x - lane.off[k], half: lane.road, sleepers: count(m) };
                b.vertex(rings[k].floor([x, road[k]]), paint, paint_at)
            };
            // On a seam's line the road has the seam's corners: where a hole's side stands on it.
            let runs = [
                rings[0].floor_run([pa[1].0, road[0]], [pa[1].1, road[0]]),
                rings[1].floor_run([pb[1].0, road[1]], [pb[1].1, road[1]]),
            ];
            if runs.iter().all(|r| r.len() == 2) {
                let v = [
                    corner(b, 0, pa[1].0, 0.0),
                    corner(b, 0, pa[1].1, 0.0),
                    corner(b, 1, pb[1].1, length),
                    corner(b, 1, pb[1].0, length),
                ];
                b.quad(v, across([0.0, 1.0]));
            } else {
                let va: Vec<u32> = runs[0].iter().map(|q| corner(b, 0, q[0], 0.0)).collect();
                let vz: Vec<u32> = runs[1].iter().map(|q| corner(b, 1, q[0], length)).collect();
                b.strip_of((&va, &runs[0]), (&vz, &runs[1]), across([0.0, 1.0]));
            }
            // Per side from the road out: the kerb's face up to the walk, and the walk to the lane's edge.
            for (side, j) in [(1.0, 2), (-1.0, 0)] {
                // The walk's edge at the road, and at the lane's edge.
                let ends = |p: (f64, f64, f64)| if side > 0.0 { (p.0, p.1) } else { (p.1, p.0) };
                let ((ra, ea), (rz, ez)) = (ends(pa[j]), ends(pb[j]));
                let (wa, wz) = (pa[j].2, pb[j].2);
                let kerb = ([[ra, pa[1].2], [ra, wa]], [[rz, pb[1].2], [rz, wz]]);
                floor_strip(b, (&kerb.0, &kerb.1), bore(inside.walk), across([-side, 0.0]));
                let walk = [rings[0].floor_run([ra, wa], [ea, wa]), rings[1].floor_run([rz, wz], [ez, wz])];
                floor_strip(b, (&walk[0], &walk[1]), bore(inside.walk), across([0.0, 1.0]));
            }
        }
    }
    // Where two lanes meet, a riser from the top of the one to the top of the other, facing the lower.
    for i in 1..chord.lanes.len() {
        let [(x, y), (z, w)] = [0, 1].map(|k| (lane_pieces(net, c, i - 1, k), lane_pieces(net, c, i, k)));
        let ([xa, ya], [xz, yz]) = ([x[x.len() - 1], y[0]], [z[z.len() - 1], w[0]]);
        let lower_left = ya.2 + yz.2 < xa.2 + xz.2;
        let (qa, qz) = ([[xa.1, xa.2], [ya.0, ya.2]], [[xz.1, xz.2], [yz.0, yz.2]]);
        floor_strip(b, (&qa, &qz), bore(inside.walk), across([if lower_left { 1.0 } else { -1.0 }, 0.0]));
    }
    // The walls stand on the floor at its sides, the vault on the walls.
    let [fa, fz] = [0, 1].map(|k| floor_pieces(net, c, k));
    let outlines = [0, 1].map(|k| inside_outline(net, chord, k, net.half_at(c, k)));
    let (o, n) = (&outlines[0], outlines[0].len());
    let feet = [[fa[0].2, fz[0].2], [fa[fa.len() - 1].2, fz[fz.len() - 1].2]];
    let lower = |k: usize| net.lower(c, k).map(|l| spring(net, chord.section, l));
    // Each face faces the middle of the room: a box section's spring is its roof.
    let to_middle = |j: usize| across(inward(o[j], o[j + 1], (chord.height[0] + chord.height[1]) / 4.0));
    for (side, foot, j) in [(0, feet[0], 0), (1, feet[1], n - 2)] {
        let sides = [0, 1].map(|k| {
            let x = outlines[k][if side == 0 { 0 } else { n - 1 }][0];
            let side = wall_side(x, foot[k], sp[k], lower(k));
            let mut corners = vec![side[0]];
            for w in side.windows(2) {
                corners.extend_from_slice(&rings[k].wall_run(w[0], w[1])[1..]);
            }
            let points: Vec<[f64; 3]> = (corners.iter().enumerate())
                .map(|(i, &q)| if i == 0 { rings[k].floor(q) } else { rings[k].ring.at(q) })
                .collect();
            (corners, points)
        });
        b.strip_at((&sides[0].1, &sides[0].0), (&sides[1].1, &sides[1].0), bore(wall), to_middle(j));
    }
    for j in 1..n - 2 {
        if rings.iter().all(|r| r.parent.is_none()) {
            b.strip((&s, &outlines[0][j..j + 2]), (&t, &outlines[1][j..j + 2]), bore(vault), to_middle(j));
            continue;
        }
        let [qa, qz] = [0, 1].map(|k| rings[k].top_run(outlines[k][j], outlines[k][j + 1]));
        b.strip((&s, &qa), (&t, &qz), bore(vault), to_middle(j));
    }
    for (k, o) in outlines.iter().enumerate() {
        step(b, net, c, k, o, vault);
    }
    // At an opening (not a closed end), the front of the floor goes down to the sill, `lift_m` under the room's floor.
    // Each side of a piece has a corner at the top of each piece beside it, which is the corner of a face there.
    let open = |k: usize| net.end_at(c, k).is_some_and(|e| !net.closed(e) && net.holes[e].part != Part::Seam);
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
            let u = (m / length).clamp(0.0, 1.0);
            let off = lane.off[0] + (lane.off[1] - lane.off[0]) * u;
            // Under the crown, lower by the fall of the vault from the axis to the lane.
            let h = chord.height[0] + (chord.height[1] - chord.height[0]) * u;
            let o = outline(chord.section, net.half_at(c, 0), h, spring(net, chord.section, h), segments(net, chord));
            let fall = top_at(&o, 0.0).zip(top_at(&o, off)).map_or(0.0, |(c, v)| c - v);
            let up = h - LAMP_DROP_M - fall;
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

/// The outer shell of a run of pieces, each its start and end station, its chord's height at each and its section,
/// with the inside half-width `half` and its arch in `segments`; its walls go down to the world height `foot`. Where
/// two pieces of different height meet, a face closes the step, facing the lower one; so too after the last piece
/// where a lower shell goes on (`meets`). There the walls have a corner at the foot of the shell that goes on.
pub fn shell<L: Fn([f64; 3]) -> f64>(
    b: &mut Builder<L>,
    net: &Net,
    pieces: &[(Station, Station, [f64; 2], Section)],
    (half, segments): (f64, usize),
    foot: f64,
    colour: u32,
    meets: Meets,
) {
    let last = |i: usize| i + 1 == pieces.len();
    let next_of = |i: usize| pieces.get(i + 1).map(|p| (p.2[0], p.3)).or(meets.lower.filter(|_| last(i)));
    let outer = |st: &Station, h: f64, section: Section| shell_outline(net, (section, segments), half, h, foot - st.z);
    for (i, &(s, t, [hs, ht], section)) in pieces.iter().enumerate() {
        let (s, t) = (&s, &t);
        let (sp, top) = ([hs, ht].map(|h| spring(net, section, h)), [hs, ht].map(|h| shell_spring(net, section, h)));
        let (os, ot) = (outer(s, hs, section), outer(t, ht, section));
        let n = os.len();
        let away = |j: usize| {
            let o = inward(os[j], os[j + 1], sp[0]);
            s.mix(t, 0.5).out([-o[0], -o[1]])
        };
        // The spring of a lower piece that goes on before or after it: a step face meets its walls there.
        let lower = |p: Option<(f64, Section)>, h: f64| {
            p.filter(|p| p.0 < h && p.1 == section).map(|p| shell_spring(net, section, p.0))
        };
        let before = lower(i.checked_sub(1).map(|k| (pieces[k].2[1], pieces[k].3)), hs);
        let after = lower(next_of(i), ht);
        let other = meets.foot.filter(|_| last(i)).map(|f| f - t.z);
        for (j, x) in [(0, os[0][0]), (n - 2, os[n - 1][0])] {
            let (a, mut z) = (wall_side(x, os[0][1], top[0], before), wall_side(x, ot[0][1], top[1], after));
            if let Some(f) = other.filter(|&f| f > z[0][1] && f < z[1][1]) {
                z.insert(1, [x, f]);
            }
            b.strip((s, &a), (t, &z), colour, away(j));
        }
        for j in 1..n - 2 {
            b.strip((s, &os[j..j + 2]), (t, &ot[j..j + 2]), colour, away(j));
        }
        let Some((next, next_section)) = next_of(i) else { continue };
        if next == ht || next_section != section {
            continue;
        }
        // The step at t: from the taller outline to the lower, facing the lower piece.
        let (h, section) = (&ht, &section);
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
    inside_outline(net, &net.chords[c], 0, net.chords[c].half)
}
