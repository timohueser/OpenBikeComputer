//! Storey 0's track and step details near the eye (`place`, in the cell build): a track's rails, sleepers, rack,
//! haul cable and its rollers, and a flight's treads or logs. Each part stands on rings: storey corners with the label
//! that the cell's block finds at their point, so a ring takes the height of the face under it as the cell mesh's
//! corners do. The viewer builds each part from its rings (`plan/details.wgsl`).
//!
//! A track's lines are its rails (its middle ± the rail middle) and, on a rack line or a funicular, its middle. A
//! line's rings are its way's samples and a point each `RING_M` of stroke metres, offset onto the line, its cuts by
//! the cell lines, and where the face under it changes: read each `PROBE_M` and found to `BISECT_M`. A span stands on
//! a rail bed or a crossing face, and a run of spans has a cap at each end but on a cell line. A line's top over its
//! face sinks to `FLUSH_M` over `RAMP_M` before the band of a ground road or path that crosses it on a track's bed:
//! continuous along the line, by the metres from its ring to the band along its tangent. A sleeper stands at each `SLEEPER_M` of stroke metres where both its seats lie on standing spans of a bed and
//! its footprint crosses no jump of the stroke metres (another tile's span, a way's end); a seat past the cell line
//! takes the ring where the sleeper's axis leaves the square. A later track (`way_key`) stands `STACK_M` lower inside
//! an earlier one's sleeper band. A flight's span splits into treads by its rise, each a box from under the face to
//! its higher end's graph height, where its rings lie on its own carriageway; or on a fraying look, its logs lie where
//! the curve crosses a height step. Every rule reads only the graph and the cell's block, each cut is reckoned from
//! the lesser end of its segment, so two cells give a part on their line alike.

use super::edges::{bbox, to_segment, way_key, way_points, Grid, Point};
use crate::faces::records::{self, find, LINE};
use crate::faces::{Rules, Surface};
use crate::frame::{metres_per_unit_at, CELL_UNITS};
use crate::graph::{point as curve_point, s_at, u_at, Family, Level, WayGraph};
use crate::sections::{Finish, Reach};

/// The kinds of a part (an item's top bits): swept along a line (rail, rack, cable), boxes (sleeper, roller, tread,
/// a swept part's end cap) and a log. As `plan/details.wgsl`.
pub const RAIL: u32 = 0;
pub const RACK: u32 = 1;
pub const CABLE: u32 = 2;
pub const SLEEPER: u32 = 3;
pub const ROLLER: u32 = 4;
pub const TREAD: u32 = 5;
pub const CAP: u32 = 6;
pub const LOG: u32 = 7;
/// The vertices of a part of each draw: a swept span (12 quads), a box (6 quads, or a cap's 10 triangles), a log.
pub const VERTICES: [u32; 3] = [72, 36, 84];

/// A ring each `RING_M` of stroke metres between the samples.
const RING_M: f64 = 4.0;
/// The face under a line is read this often along it, and a change found to this (metres).
const PROBE_M: f64 = 1.0;
const BISECT_M: f64 = 0.05;
/// A rail's middle from its track's middle, and half a sleeper: standard and narrow gauge (`roads/parts.ts`).
const GAUGE_M: [f64; 2] = [0.7525, 0.53];
const SLEEPER_HALF_M: [f64; 2] = [1.3, 0.9];
/// A sleeper's place each `SLEEPER_M` of stroke metres and its half-width along the track; a roller's.
const SLEEPER_M: f64 = 0.6;
const SLEEPER_ALONG_M: f64 = 0.13;
const ROLLER_M: f64 = 8.0;
const ROLLER_ALONG_M: f64 = 0.05;
/// A later track stands this much lower inside an earlier one's sleeper band.
const STACK_M: f64 = 0.01;
/// A line's top over its face at a crossing, and the metres over which it rises to standing. A crossing's band is the
/// carriageway, which wins over a bed, about the graph's chord, and `SAG_M`: a chord leaves its curve by less.
const FLUSH_M: f64 = 0.004;
const RAMP_M: f64 = 3.0;
const SAG_M: f64 = 0.1;
/// A line's top over its face where it stands: a sleeper's top and a rail, rack or cable (`plan/details.wgsl`).
const SLEEPER_UP_M: f64 = 0.05;
const TOP_M: [f64; 3] = [0.125, 0.09, 0.09];
const RAIL_SCALE: f64 = 1.2;
/// A sleeper's seat lies this near its rail's span (metres): the chord of the rail leaves its curve by less.
const SEAT_M: f64 = 0.1;
/// In a sleeper's second word: its second seat is the ring where its axis leaves the cell square.
const AXIS: u32 = 1;
/// Town steps: the rise of a step, the least and most tread, and how far a tread's top lies over its higher end.
const RISE_M: f64 = 0.16;
const TREAD_MIN_M: f64 = 0.26;
const TREAD_MAX_M: f64 = 1.6;
const TREAD_LIFT_M: f64 = 0.04;
/// Trail steps: a log each `LOG_RISE_M` of height, never nearer than `LOG_MIN_M`, its shift along the way at most
/// (its turn is a share of LOG_TURN of `plan/details.wgsl`); the half-lengths of the logs (`roads/parts.ts`); a
/// trail's tread and a gravel path's fray (`roads/looks.wgsl`).
const LOG_RISE_M: f64 = 0.2;
const LOG_MIN_M: f64 = 0.9;
const LOG_SHIFT_M: f64 = 0.06;
const LOG_SEED: u64 = 31;
const LOG_HALF_M: [f64; 6] = [0.3, 0.35, 0.45, 0.55, 0.7, 0.85];
const TRAIL_TREAD_M: f64 = 0.35;
const GRAVEL_FRAY_M: f64 = 0.15;

/// A ring: its point (frame units from the cell's origin), the label of the face there, its height offset, the unit
/// tangent of its line (frame axes) and its stroke metres; a tread's ring holds the metres from its nosing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ring {
    pub p: [i32; 2],
    pub label: u32,
    pub offset: f64,
    pub tangent: [f64; 2],
    pub along: f64,
}

/// A part: its level-18 node in the cell (row by row from the north-west) and its two words (`plan/details.wgsl`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Item {
    pub node: u8,
    pub words: [u32; 2],
}

impl Item {
    pub fn kind(&self) -> u32 {
        self.words[0] >> 28
    }

    /// Its draw: an index of `VERTICES`.
    pub fn draw(&self) -> usize {
        match self.kind() {
            RAIL | RACK | CABLE => 0,
            LOG => 2,
            _ => 1,
        }
    }

    /// Its first ring.
    pub fn ring(&self) -> usize {
        (self.words[0] & 0xffff) as usize
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Details {
    pub rings: Vec<Ring>,
    pub items: Vec<Item>,
}

/// The row of the reach table: how far past the cell square a part reads the graph (a seat past the line, a ring).
pub fn reach() -> Reach {
    let terms =
        [("2·SLEEPER_HALF_M", 2.0 * SLEEPER_HALF_M[0]), ("SLEEPER_ALONG_M", SLEEPER_ALONG_M), ("RING_M", RING_M)];
    let terms = terms.iter().map(|&(what, m)| (what.to_string(), m)).collect();
    Reach { name: "details".into(), owner: "details", chain: false, terms }
}

type V = [f64; 2];

fn lerp(a: V, b: V, f: f64) -> V {
    [a[0] + f * (b[0] - a[0]), a[1] + f * (b[1] - a[1])]
}

fn dist(a: V, b: V) -> f64 {
    libm::hypot(a[0] - b[0], a[1] - b[1])
}

fn unit(v: V) -> V {
    let l = libm::hypot(v[0], v[1]).max(1e-12);
    [v[0] / l, v[1] / l]
}

/// The left of a direction in frame axes (y south), seen from above.
fn left(t: V) -> V {
    [t[1], -t[0]]
}

fn rounded(q: V) -> V {
    q.map(libm::round)
}

/// A point of a line: its place, its unit tangent, its stroke metres and the graph's height.
#[derive(Clone, Copy, Debug)]
struct At {
    p: V,
    t: V,
    along: f64,
    z: f64,
}

fn mix(a: &At, b: &At, f: f64) -> At {
    At {
        p: lerp(a.p, b.p, f),
        t: unit(lerp(a.t, b.t, f)),
        along: a.along + f * (b.along - a.along),
        z: a.z + f * (b.z - a.z),
    }
}

fn on_line(q: V) -> bool {
    let c = CELL_UNITS as f64;
    q[0] == 0.0 || q[1] == 0.0 || q[0] == c || q[1] == c
}

fn in_cell(q: V) -> bool {
    let c = CELL_UNITS as f64;
    (0.0..c).contains(&q[0]) && (0.0..c).contains(&q[1])
}

/// The pieces of segment a → b (rounded points) in the closed cell square, in its order. A cut point lies on its
/// cell line, the other coordinate rounded, reckoned from the lesser end: two cells and either direction cut alike.
fn pieces(a: &At, b: &At) -> Vec<(At, At)> {
    let c = CELL_UNITS as f64;
    let (p, r, swap) = if a.p < b.p { (a, b, false) } else { (b, a, true) };
    let mut cuts = vec![(0.0, *p), (1.0, *r)];
    for (axis, bound) in [(0, 0.0), (0, c), (1, 0.0), (1, c)] {
        let (u, v) = (p.p[axis] - bound, r.p[axis] - bound);
        if u * v < 0.0 {
            let f = (bound - p.p[axis]) / (r.p[axis] - p.p[axis]);
            let mut q = mix(p, r, f);
            q.p = [0, 1].map(|k| if k == axis { bound } else { libm::round(q.p[k]) });
            cuts.push((f, q));
        }
    }
    cuts.sort_by(|x, y| x.0.total_cmp(&y.0));
    let closed = |q: V| (0.0..=c).contains(&q[0]) && (0.0..=c).contains(&q[1]);
    let mut out: Vec<(At, At)> = (cuts.windows(2))
        .filter(|w| w[1].0 > w[0].0 && w[0].1.p != w[1].1.p && closed(lerp(w[0].1.p, w[1].1.p, 0.5)))
        .map(|w| (w[0].1, w[1].1))
        .collect();
    if swap {
        out.reverse();
        out.iter_mut().for_each(|x| *x = (x.1, x.0));
    }
    out
}

/// What a line stands on at a point: nothing, a rail bed, or a crossing face.
#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
    None,
    Bed,
    Crossing,
}

/// A station of a line in the cell: its point, its state and label there, and whether the line leaves the cell
/// square after it.
#[derive(Clone, Copy, Debug)]
struct Station {
    at: At,
    state: State,
    label: u32,
    gap: bool,
}

/// A standing span of a bed: its first ring in the table (the next ring is its other end) and its ends.
#[derive(Clone, Copy, Debug)]
struct Span {
    ring: u32,
    a: V,
    b: V,
    along: [f64; 2],
}

impl Span {
    fn lo(&self) -> f64 {
        self.along[0].min(self.along[1])
    }
}

/// A track's look: narrow, timber sleepers, a rack, a cable, its finish code.
#[derive(Clone, Copy)]
struct Gauge {
    narrow: bool,
    timber: bool,
    rack: bool,
    cable: bool,
    code: u32,
}

struct Cx<'a> {
    g: &'a WayGraph,
    words: &'a [u32],
    origin_y: f64,
    out: Details,
    /// The chords of the ground tracks, each with its sleeper half (metres) and its way's `way_key`.
    tracks: (Grid, Vec<([V; 2], f64, u32)>),
    /// The chords of the ground roads and paths, each with its band (metres), and the widest band.
    roads: (Grid, Vec<([V; 2], f64)>, f64),
}

impl Cx<'_> {
    fn mpu(&self, q: V) -> f64 {
        metres_per_unit_at(self.origin_y + q[1])
    }

    /// The node of a point: its level-18 square of the cell, row by row.
    fn node(q: V) -> u8 {
        let s = CELL_UNITS as f64 / 4.0;
        let k = |v: f64| (libm::floor(v / s) as i32).clamp(0, 3) as u8;
        k(q[1]) * 4 + k(q[0])
    }

    fn bed(&self, q: V) -> (State, u32) {
        let Some(l) = find(self.words, q) else { return (State::None, super::NO_LABEL) };
        let s = records::label(self.words, l)[0] & 0xff;
        let state = match s {
            x if x == Surface::RailBed as u32 => State::Bed,
            x if x == Surface::Crossing as u32 => State::Crossing,
            _ => State::None,
        };
        (state, l)
    }

    /// The label at q where it is way `id`'s own carriageway.
    fn own(&self, q: V, id: u32) -> Option<u32> {
        let l = find(self.words, q)?;
        let w = records::label(self.words, l);
        let line = w[2];
        (w[0] & 0xff == Surface::Carriageway as u32
            && line != u32::MAX
            && self.words[self.words[6] as usize + LINE * line as usize] == id)
            .then_some(l)
    }

    /// Whether q lies in the sleeper band of a ground track earlier than `key` in `way_key` order.
    fn lowered(&self, q: V, key: u32) -> bool {
        let (grid, chords) = &self.tracks;
        let mpu = self.mpu(q);
        grid.any(q, SLEEPER_HALF_M[0] / mpu, |k| {
            let ([a, b], half, other) = chords[k as usize];
            other < key && to_segment(q, a, b) * mpu <= half
        })
    }

    /// The metres along a line from q (on a ground track's line, unit tangent t) to the nearest band of a ground road
    /// or path that crosses it on a track's bed: 0 inside one. The line meets a chord's line at x, |s| from q at the
    /// angle θ, so the band begins band / sin θ before x. A chord counts where x lies on it or within its band past an
    /// end, and in a ground track's sleeper band: not past a deck's end.
    fn crossing_m(&self, q: V, t: V) -> f64 {
        let (grid, chords, widest) = &self.roads;
        let mpu = self.mpu(q);
        let mut best = f64::INFINITY;
        grid.any(q, (RAMP_M + 3.0 * widest) / mpu, |k| {
            let ([a, b], band) = chords[k as usize];
            if to_segment(q, a, b) * mpu <= band {
                best = 0.0;
                return true;
            }
            let u = [b[0] - a[0], b[1] - a[1]];
            let (cross, len) = (t[0] * u[1] - t[1] * u[0], libm::hypot(u[0], u[1]));
            if cross.abs() < 1e-9 * len {
                return false;
            }
            let w = [a[0] - q[0], a[1] - q[1]];
            let (s, r) = ((w[0] * u[1] - w[1] * u[0]) / cross, (w[0] * t[1] - w[1] * t[0]) / cross);
            let past = band / mpu / len;
            let x = [q[0] + s * t[0], q[1] + s * t[1]];
            if (-past..=1.0 + past).contains(&r) && self.on_track(x) {
                best = best.min((s.abs() * mpu - band * len / cross.abs()).max(0.0));
            }
            false
        });
        best
    }

    fn on_track(&self, q: V) -> bool {
        let (grid, chords) = &self.tracks;
        let mpu = self.mpu(q);
        grid.any(q, SLEEPER_HALF_M[0] / mpu, |k| {
            to_segment(q, chords[k as usize].0[0], chords[k as usize].0[1]) * mpu <= chords[k as usize].1
        })
    }

    /// A ring's offset: lower inside an earlier track's sleeper band, and sunk toward a crossing band from a line's
    /// top over its face `top_m` to `FLUSH_M`.
    fn offset_at(&self, at: &At, key: u32, top_m: f64) -> f64 {
        let ramp = (self.crossing_m(at.p, at.t) / RAMP_M).min(1.0);
        let stack = if self.lowered(at.p, key) { -STACK_M } else { 0.0 };
        stack - (top_m - FLUSH_M) * (1.0 - ramp)
    }

    fn ring(&mut self, at: &At, label: u32, offset: f64) -> u32 {
        let p = at.p.map(|v| libm::round(v) as i32);
        self.out.rings.push(Ring { p, label, offset, tangent: at.t, along: at.along });
        self.out.rings.len() as u32 - 1
    }

    /// An item, which names its rings in 16 bits: none past that many rings.
    fn item(&mut self, q: V, words: [u32; 2]) {
        if self.out.rings.len() <= 1 << 16 {
            self.out.items.push(Item { node: Cx::node(q), words });
        }
    }

    /// The stations of a piece a → b (rounded points): its ends, and two at each change of the state between reads
    /// each `PROBE_M`, found to `BISECT_M`, at rounded points; reckoned from the lesser end.
    fn probe(&self, a: &At, b: &At, out: &mut Vec<Station>) {
        let (p, r, swap) = if a.p < b.p { (a, b, false) } else { (b, a, true) };
        let len = dist(p.p, r.p) * self.mpu(p.p);
        let n = libm::ceil(len / PROBE_M).max(1.0) as usize;
        let at = |f: f64| {
            let mut x = mix(p, r, f);
            x.p = rounded(x.p);
            x
        };
        let station = |x: At| {
            let (state, label) = self.bed(x.p);
            Station { at: x, state, label, gap: false }
        };
        let mut st = vec![station(*p)];
        let mut last = st[0];
        for k in 1..=n {
            let next = station(if k == n { *r } else { at(k as f64 / n as f64) });
            if next.state != last.state {
                let (mut lo, mut hi) = ((k - 1) as f64 / n as f64, k as f64 / n as f64);
                let (mut a, mut b) = (last, next);
                while (hi - lo) * len > BISECT_M {
                    let m = station(at((lo + hi) / 2.0));
                    if m.state == a.state {
                        (lo, a) = ((lo + hi) / 2.0, m);
                    } else {
                        (hi, b) = ((lo + hi) / 2.0, m);
                    }
                }
                for s in [a, b] {
                    if s.at.p != st.last().unwrap().at.p && s.at.p != r.p {
                        st.push(s);
                    }
                }
            }
            last = next;
        }
        st.push(last);
        if swap {
            st.reverse();
        }
        let start = out.last().map(|s| s.at.p);
        if start == Some(st[0].at.p) {
            st.remove(0);
        } else if let Some(last) = out.last_mut() {
            last.gap = true;
        }
        out.extend(st);
    }

    /// The runs of a line (its points in order): per run its rings, a span item between each two and a cap at each
    /// end off the cell line. Returns its standing spans on a bed by their least stroke metres.
    fn line(&mut self, pts: &[At], kind: u32, flags: u32, code: u32, key: u32) -> Vec<Span> {
        let mut st: Vec<Station> = Vec::new();
        for w in pts.windows(2) {
            for (a, b) in pieces(&w[0], &w[1]) {
                self.probe(&a, &b, &mut st);
            }
        }
        let stands = |i: usize| st[i].state != State::None && st[i + 1].state != State::None && !st[i].gap;
        let scale = if kind == RAIL && flags & 2 == 0 { RAIL_SCALE } else { 1.0 };
        let top = SLEEPER_UP_M + TOP_M[kind as usize] * scale;
        let mut spans = Vec::new();
        let mut i = 0;
        while i + 1 < st.len() {
            if !stands(i) {
                i += 1;
                continue;
            }
            let mut j = i;
            while j + 1 < st.len() && stands(j) {
                j += 1;
            }
            let first = self.out.rings.len() as u32;
            for s in &st[i..=j] {
                let offset = self.offset_at(&s.at, key, top);
                self.ring(&s.at, s.label, offset);
            }
            for k in i..j {
                let r = first + (k - i) as u32;
                self.item(lerp(st[k].at.p, st[k + 1].at.p, 0.5), [word(kind, r, flags), code]);
                if st[k].state == State::Bed && st[k + 1].state == State::Bed {
                    let along = [st[k].at.along, st[k + 1].at.along];
                    spans.push(Span { ring: r, a: st[k].at.p, b: st[k + 1].at.p, along });
                }
            }
            for (end, k, r) in [(0, i, first), (1, j, first + (j - i) as u32)] {
                if !on_line(st[k].at.p) {
                    self.item(st[k].at.p, [word(CAP, r, flags | end << 3 | kind << 4), code]);
                }
            }
            i = j;
        }
        spans.sort_by(|x, y| x.lo().total_cmp(&y.lo()));
        spans
    }

    /// The seat at q on the nearest span of `lines` about stroke metres m, within `SEAT_M`: its ring and share.
    fn seat(&self, lines: &[Vec<Span>], q: V, m: f64) -> Option<(u32, f64)> {
        // Each line's spans by their least stroke metres; a span is at most `RING_M` long.
        let d = |s: &Span| to_segment(q, s.a, s.b);
        let mut best: Option<&Span> = None;
        for spans in lines {
            let (from, to) =
                (spans.partition_point(|s| s.lo() < m - 1.5 - RING_M), spans.partition_point(|s| s.lo() <= m + 0.5));
            for s in spans[from..to].iter().filter(|s| s.along[0].max(s.along[1]) >= m - 0.5) {
                if best.is_none_or(|b| d(s) < d(b)) {
                    best = Some(s);
                }
            }
        }
        let best = best?;
        let v = [best.b[0] - best.a[0], best.b[1] - best.a[1]];
        let t = ((q[0] - best.a[0]) * v[0] + (q[1] - best.a[1]) * v[1]) / (v[0] * v[0] + v[1] * v[1]).max(1e-12);
        (d(best) * self.mpu(q) <= SEAT_M).then_some((best.ring, t.clamp(0.0, 1.0)))
    }

    /// A track's lines, sleepers and rollers.
    fn track(&mut self, w: u32, gauge: Gauge, key: u32) {
        let mut pts = Vec::new();
        way_points(self.g, w, 1, RING_M, &mut pts);
        let line = |off: f64, cx: &Cx| -> Vec<At> { pts.iter().map(|p| cx.offset(p, off)).collect() };
        let g = GAUGE_M[usize::from(gauge.narrow)];
        let narrow = u32::from(gauge.narrow) << 1;
        let (l, r) = (line(g, self), line(-g, self));
        let rails = [self.line(&l, RAIL, narrow | 1, gauge.code, key), self.line(&r, RAIL, narrow, gauge.code, key)];
        let middle = line(0.0, self);
        if gauge.rack {
            self.line(&middle, RACK, narrow, gauge.code, key);
        }
        let cable = if gauge.cable { self.line(&middle, CABLE, narrow, gauge.code, key) } else { Vec::new() };
        let c = CELL_UNITS as f64;
        let closed = |s: V| (0.0..=c).contains(&s[0]) && (0.0..=c).contains(&s[1]);
        let seat = |(ring, t): (u32, f64)| ring | (libm::round(t * 4095.0) as u32) << 16;
        let flags = narrow | u32::from(gauge.timber) << 2;
        for (m, q, t, _) in marks(self.g, w, SLEEPER_M, SLEEPER_ALONG_M).into_iter().filter(|x| in_cell(x.1)) {
            let (n, off) = (left(t), g / self.mpu(q));
            let seats = [[q[0] + n[0] * off, q[1] + n[1] * off], [q[0] - n[0] * off, q[1] - n[1] * off]];
            let inside = seats.map(closed);
            let refs = [0, 1].map(|k| inside[k].then(|| self.seat(&rails, seats[k], m)).flatten());
            match (refs, inside) {
                ([Some(a), Some(b)], _) => self.item(q, [seat(a) | SLEEPER << 28, seat(b) | flags << 28]),
                ([Some(a), None], [true, false]) | ([None, Some(a)], [false, true]) => {
                    let (near, far) = if inside[0] { (seats[0], seats[1]) } else { (seats[1], seats[0]) };
                    let x = exit(near, far);
                    let (state, label) = self.bed(x);
                    if state == State::Bed {
                        let at = At { p: x, t, along: m, z: 0.0 };
                        let offset = self.offset_at(
                            &at,
                            key,
                            SLEEPER_UP_M + TOP_M[0] * if gauge.narrow { 1.0 } else { RAIL_SCALE },
                        );
                        let ring = self.ring(&at, label, offset);
                        self.item(q, [seat(a) | SLEEPER << 28, ring | (AXIS | flags) << 28]);
                    }
                }
                _ => {}
            }
        }
        if gauge.cable {
            for (m, q, ..) in marks(self.g, w, ROLLER_M, ROLLER_ALONG_M).into_iter().filter(|x| in_cell(x.1)) {
                if let Some(a) = self.seat(std::slice::from_ref(&cable), q, m) {
                    self.item(q, [seat(a) | ROLLER << 28, gauge.code]);
                }
            }
        }
    }

    /// A point of a way offset `off_m` to its left: the way's point and the offset each rounded, so two cells give it
    /// alike (a post point between samples is the curve's, not a whole unit).
    fn offset(&self, p: &Point, off_m: f64) -> At {
        let (n, k) = (left(p.t), off_m / self.mpu(p.pos));
        let q = [0, 1].map(|i| libm::round(p.pos[i]) + libm::round(n[i] * k));
        At { p: q, t: p.t, along: p.along, z: p.z }
    }

    /// A flight's treads per span of the graph (`RISE_M`): a box between two rings on its middle line whose offsets
    /// lift it to its higher end's graph height plus `TREAD_LIFT_M`, where both stand on its own carriageway.
    fn treads(&mut self, w: u32, half: f64) {
        let g = self.g;
        let id = g.ways[w as usize].id;
        for (k, s0, s1) in spans_of(g, w) {
            let at = |s: f64| curve_at(g, w, k, s);
            let (a, b) = (at(s0), at(s1));
            let (len, rise) = (s1 - s0, (b.z - a.z).abs());
            if len < 0.1 {
                continue;
            }
            let count = libm::round(rise / RISE_M).max(1.0);
            let tread = (len / count).clamp(TREAD_MIN_M, TREAD_MAX_M);
            let n = libm::round(len / tread).max(1.0) as usize;
            for j in 0..n {
                let mut ends = [j, j + 1].map(|e| at(s0 + e as f64 * len / n as f64));
                ends.iter_mut().for_each(|e| e.p = rounded(e.p));
                let level = ends[0].z.max(ends[1].z) + TREAD_LIFT_M;
                let nose = if ends[0].z <= ends[1].z { ends[0].p } else { ends[1].p };
                for (pa, pb) in pieces(&ends[0], &ends[1]) {
                    let (Some(la), Some(lb)) = (self.own(pa.p, id), self.own(pb.p, id)) else { continue };
                    let mpu = self.mpu(pa.p);
                    let first = self.out.rings.len() as u32;
                    for (x, l) in [(pa, la), (pb, lb)] {
                        let at = At { along: dist(nose, x.p) * mpu, ..x };
                        self.ring(&at, l, level - x.z);
                    }
                    let open = u32::from(pa.p != ends[0].p && on_line(pa.p)) << 1
                        | u32::from(pb.p != ends[1].p && on_line(pb.p)) << 2;
                    self.item(lerp(pa.p, pb.p, 0.5), [word(TREAD, first, open), (half as f32).to_bits()]);
                }
            }
        }
    }

    /// A flight's logs per span of the graph (`logs_of`), each on a ring of its own carriageway, its length within
    /// the tread `tread_m`.
    fn logs(&mut self, w: u32, tread_m: f64) {
        let g = self.g;
        let way = &g.ways[w as usize];
        let fits = LOG_HALF_M.iter().filter(|&&h| h <= tread_m).count().max(1);
        let samples = &g.samples[way.samples.start as usize..way.samples.end as usize];
        for (k, s0, s1) in spans_of(g, w) {
            let inner = samples.iter().filter(|s| s.span == k && s.s_m > s0 && s.s_m < s1).map(|s| s.s_m);
            let st: Vec<(f64, f64)> =
                [s0].into_iter().chain(inner).chain([s1]).map(|s| (s, curve_at(g, w, k, s).z)).collect();
            for (s, level, grade) in logs_of(&st) {
                let random =
                    |salt: u64| (splitmix64(level as u64 ^ salt << 32 ^ LOG_SEED) >> 11) as f64 / (1u64 << 53) as f64;
                let shift = LOG_SHIFT_M * (2.0 * random(1) - 1.0) * grade.signum();
                let mut at = curve_at(g, w, k, (s + shift).clamp(s0, s1));
                at.p = rounded(at.p);
                if !in_cell(at.p) {
                    continue;
                }
                let Some(label) = self.own(at.p, way.id) else { continue };
                let part = (fits - 1).saturating_sub(libm::floor(2.0 * random(2)) as usize) as u32;
                let turn = 2.0 * random(3) - 1.0;
                let ring = self.ring(&at, label, 0.0);
                let turn = pack_snorm(turn, (grade / 4.0).clamp(-1.0, 1.0));
                self.item(at.p, [word(LOG, ring, part), turn]);
            }
        }
    }
}

/// An item's first word: its kind, its first ring and 12 bits of its own.
fn word(kind: u32, ring: u32, bits: u32) -> u32 {
    ring & 0xffff | (bits & 0xfff) << 16 | kind << 28
}

/// `pack2x16snorm` of WGSL.
pub(super) fn pack_snorm(x: f64, y: f64) -> u32 {
    let q = |v: f64| (libm::round(v.clamp(-1.0, 1.0) * 32767.0) as i32 as u32) & 0xffff;
    q(x) | q(y) << 16
}

fn splitmix64(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Where the segment from `near` (in the closed square) toward `far` leaves the cell square, rounded and one unit
/// inside it: no ring of a sleeper's axis lies on a cell line, where the neighbour has no twin.
fn exit(near: V, far: V) -> V {
    let c = CELL_UNITS as f64;
    let mut f: f64 = 1.0;
    for axis in 0..2 {
        let d = far[axis] - near[axis];
        if d > 0.0 {
            f = f.min((c - near[axis]) / d);
        } else if d < 0.0 {
            f = f.min(-near[axis] / d);
        }
    }
    rounded(lerp(near, far, f)).map(|v| v.clamp(1.0, c - 1.0))
}

/// Per span of way w that the graph holds: its index and the way's span metres in it.
fn spans_of(g: &WayGraph, w: u32) -> Vec<(u32, f64, f64)> {
    let way = &g.ways[w as usize];
    let part = &g.parts[way.part as usize];
    let mut ks: Vec<u32> =
        g.samples[way.samples.start as usize..way.samples.end as usize].iter().map(|s| s.span).collect();
    ks.dedup();
    ks.into_iter()
        .filter_map(|k| {
            let span = &part.spans[k as usize];
            let s0 = if k == way.from.span { s_at(span, way.from.u) } else { 0.0 };
            let s1 = if k == way.to.span { s_at(span, way.to.u) } else { span.len_m };
            (k >= way.from.span && k <= way.to.span && s1 > s0).then_some((k, s0, s1))
        })
        .collect()
}

/// The point of way w at metres s of span k: frame units, unrounded.
fn curve_at(g: &WayGraph, w: u32, k: u32, s: f64) -> At {
    let part = &g.parts[g.ways[w as usize].part as usize];
    let span = &part.spans[k as usize];
    let p = curve_point(part, k as usize, u_at(span, s));
    let (v, o) = (part.vertices[k as usize].pos, g.cell.origin());
    let sign = if part.backward { -1.0 } else { 1.0 };
    At {
        p: [(v[0] - o.x) as f64 + p.d[0], (v[1] - o.y) as f64 + p.d[1]],
        t: p.tangent,
        along: span.along0_m + sign * s,
        z: p.z_m,
    }
}

/// The marks of way w at each `step` of its stroke metres whose footprint, `half` metres each way, lies on the way in
/// spans of continuous stroke metres: per mark its metres, point, unit tangent and the graph's height.
fn marks(g: &WayGraph, w: u32, step: f64, half: f64) -> Vec<(f64, V, V, f64)> {
    let way = &g.ways[w as usize];
    let part = &g.parts[way.part as usize];
    let sign = if part.backward { -1.0 } else { 1.0 };
    let range = |k: u32| {
        let span = &part.spans[k as usize];
        let s0 = if k == way.from.span { s_at(span, way.from.u) } else { 0.0 };
        let s1 = if k == way.to.span { s_at(span, way.to.u) } else { span.len_m };
        (k >= way.from.span && k <= way.to.span && s1 > s0).then_some((s0, s1))
    };
    // Whether the stroke metres run on from span k into the next (by `d`, ±1).
    let joins = |k: u32, d: i64| {
        let n = k as i64 + d;
        if n < 0 || n as usize >= part.spans.len() || range(n as u32).is_none() {
            return false;
        }
        let (a, b) = (&part.spans[k as usize], &part.spans[n as usize]);
        let end = |s: &crate::graph::Span| s.along0_m + sign * s.len_m;
        if d > 0 {
            (end(a) - b.along0_m).abs() <= 1e-3
        } else {
            (end(b) - a.along0_m).abs() <= 1e-3
        }
    };
    let mut out = Vec::new();
    for (k, s0, s1) in spans_of(g, w) {
        let span = &part.spans[k as usize];
        let lo = span.along0_m + sign * s0;
        let hi = span.along0_m + sign * s1;
        let (lo, hi) = (lo.min(hi), lo.max(hi));
        for j in libm::ceil(lo / step) as i64..=libm::floor(hi / step) as i64 {
            let m = j as f64 * step;
            let s = (m - span.along0_m) * sign;
            // A mark on a span's end belongs to the span after it on the way.
            if s < s0 || s > s1 || (s == s1 && k < way.to.span) {
                continue;
            }
            let fits = |x: f64| {
                (s0..=s1).contains(&x)
                    || (x < s0 && s0 == 0.0 && joins(k, -1))
                    || (x > s1 && s1 == span.len_m && joins(k, 1))
            };
            if fits(s - half) && fits(s + half) {
                let a = curve_at(g, w, k, s);
                out.push((m, a.p, a.t, a.z));
            }
        }
    }
    out
}

/// The logs of a flight's span from its stations (span metres, graph height): where the curve crosses a height
/// k·`LOG_RISE_M`, for each k that is a multiple of the m that the grade there needs to keep logs `LOG_MIN_M` apart.
/// Two crossings of one height nearer than `LOG_MIN_M` lay none; a span that crosses a height and keeps no log lays
/// one at the height nearest its middle. Per log its metres, its height step and the grade there.
fn logs_of(st: &[(f64, f64)]) -> Vec<(f64, i64, f64)> {
    let mut all: Vec<(f64, i64, i64, f64)> = Vec::new();
    for j in 0..st.len().saturating_sub(1) {
        let ((sa, za), (sb, zb)) = (st[j], st[j + 1]);
        let grade = (zb - za) / (sb - sa);
        if grade.is_nan() || grade.abs() <= 1e-6 {
            continue;
        }
        let m = libm::ceil(LOG_MIN_M * grade.abs() / LOG_RISE_M - 1e-6).max(1.0) as i64;
        let mut k = libm::ceil(za.min(zb) / LOG_RISE_M) as i64;
        while k as f64 * LOG_RISE_M <= za.max(zb) {
            let a = sa + (k as f64 * LOG_RISE_M - za) / grade;
            // A crossing at a station belongs to the segment after it, the span's end to its last segment.
            if a >= sa && (a < sb || (j + 2 == st.len() && a <= sb)) {
                all.push((a, k, m, grade));
            }
            k += 1;
        }
    }
    let lone: Vec<usize> = (0..all.len())
        .filter(|&i| !(0..all.len()).any(|j| j != i && all[j].1 == all[i].1 && (all[j].0 - all[i].0).abs() < LOG_MIN_M))
        .collect();
    let kept: Vec<(f64, i64, f64)> =
        lone.iter().map(|&i| all[i]).filter(|x| x.1.rem_euclid(x.2) == 0).map(|x| (x.0, x.1, x.3)).collect();
    if !kept.is_empty() || lone.is_empty() {
        return kept;
    }
    let (lo, hi) = lone.iter().fold((i64::MAX, i64::MIN), |(a, b), &i| (a.min(all[i].1), b.max(all[i].1)));
    let middle = (lo + hi) as f64 / 2.0;
    let off = |i: usize| (all[i].1 as f64 - middle).abs() + all[i].1 as f64 * 1e-9;
    let best = lone.iter().copied().min_by(|&a, &b| off(a).total_cmp(&off(b))).expect("a lone log");
    vec![(all[best].0, all[best].1, all[best].3)]
}

/// The details about a cell from its graph and its block: see the module.
pub fn place(g: &WayGraph, input: &crate::CellInput, rules: &Rules, words: &[u32]) -> Details {
    let section = |w: usize| {
        let f = &input.features[g.parts[g.ways[w].part as usize].feature.0 as usize];
        rules.sections.way(&f.kind, &f.tags, false)
    };
    let ground = |w: usize, family: Family| {
        let way = &g.ways[w];
        way.family == family && way.level == Level::Ground && way.samples.len() >= 2
    };
    let tracks: Vec<usize> = (0..g.ways.len()).filter(|&w| ground(w, Family::Rail)).collect();
    let steps: Vec<usize> =
        (0..g.ways.len()).filter(|&w| ground(w, Family::Path) && section(w).class == "steps").collect();
    if tracks.is_empty() && steps.is_empty() {
        return Details::default();
    }
    let mut order: Vec<u32> = (0..g.ways.len() as u32).collect();
    order.sort_by_key(|&x| way_key(g, x));
    let mut key = vec![0; g.ways.len()];
    order.iter().enumerate().for_each(|(i, &w)| key[w as usize] = i as u32);
    let mut chords = Vec::new();
    for &w in &tracks {
        let narrow = matches!(section(w).finish, Some(Finish::Narrow | Finish::NarrowRack));
        let samples = &g.samples[g.ways[w].samples.start as usize..g.ways[w].samples.end as usize];
        for s in samples.windows(2).filter(|s| s[1].span <= s[0].span + 1) {
            chords.push(([s[0].pos, s[1].pos].map(|p| p.map(f64::from)), SLEEPER_HALF_M[usize::from(narrow)], key[w]));
        }
    }
    let boxes: Vec<[f64; 4]> = chords.iter().map(|c| bbox(&c.0)).collect();
    let beds = (Grid::of(8192.0, &boxes), chords);
    let mut roads = Vec::new();
    for w in (0..g.ways.len()).filter(|&w| ground(w, Family::Road) || ground(w, Family::Path)) {
        let s = section(w);
        let samples = &g.samples[g.ways[w].samples.start as usize..g.ways[w].samples.end as usize];
        for p in samples.windows(2).filter(|p| p[1].span <= p[0].span + 1) {
            let band = s.half_m + p[0].widen_m.max(p[1].widen_m) + SAG_M;
            roads.push(([p[0].pos, p[1].pos].map(|q| q.map(f64::from)), band));
        }
    }
    let widest = roads.iter().map(|r| r.1).fold(0.0, f64::max);
    let boxes: Vec<[f64; 4]> = roads.iter().map(|c| bbox(&c.0)).collect();
    let roads = (Grid::of(8192.0, &boxes), roads, widest);
    let origin_y = input.key.origin().y as f64;
    let mut cx = Cx { g, words, origin_y, out: Details::default(), tracks: beds, roads };
    for w in tracks {
        let s = section(w);
        let Some(finish) = s.finish else { continue };
        let narrow = matches!(finish, Finish::Narrow | Finish::NarrowRack);
        let rack = matches!(finish, Finish::Rack | Finish::NarrowRack) || s.class == "rack";
        let (timber, cable, code) =
            (matches!(s.class, "funicular" | "rack"), s.class == "funicular", finish.code().into());
        let gauge = Gauge { narrow, timber, rack, cable, code };
        cx.track(w as u32, gauge, key[w]);
    }
    for w in steps {
        let s = section(w);
        if s.frays() {
            let tread = if s.finish == Some(Finish::Gravel) {
                s.half_m - GRAVEL_FRAY_M
            } else {
                TRAIL_TREAD_M.min(s.half_m - 0.05)
            };
            cx.logs(w as u32, tread);
        } else {
            cx.treads(w as u32, s.half_m.min((0.8 * s.half_m).clamp(0.6, 1.4)));
        }
    }
    cx.out
}

/// The faults of a cell's details: a ring that an item names past the table, a run of swept spans whose end lies
/// off the cell line with no cap, a tread open off the cell line.
pub fn closure(d: &Details) -> Vec<String> {
    let mut out = Vec::new();
    let line = |r: usize| d.rings.get(r).is_some_and(|x| on_line(x.p.map(f64::from)));
    let sweep = |i: &Item| i.kind() <= CABLE;
    // A swept span's line: its kind and its first flags (side, narrow).
    let key = |i: &Item, r: usize| (r, i.kind(), (i.words[0] >> 16) & 7);
    let mut spans: Vec<(usize, u32, u32)> = d.items.iter().filter(|i| sweep(i)).map(|i| key(i, i.ring())).collect();
    spans.sort_unstable();
    let mut caps: Vec<(usize, u32, u32)> = (d.items.iter().filter(|i| i.kind() == CAP))
        .map(|i| (i.ring(), (i.words[0] >> 20) & 3, (i.words[0] >> 16) & 7))
        .collect();
    caps.sort_unstable();
    for i in &d.items {
        let r = i.ring();
        if r + 1 >= d.rings.len() + usize::from(i.kind() == CAP || i.kind() == LOG) {
            out.push(format!("{:?}: ring {r} past the table", i.words));
            continue;
        }
        if sweep(i) {
            let (_, kind, flags) = key(i, r);
            for (end, next) in [(r, r.checked_sub(1)), (r + 1, Some(r + 1))] {
                let goes_on = next.is_some_and(|n| spans.binary_search(&(n, kind, flags)).is_ok() && n != r);
                if !goes_on && !line(end) && caps.binary_search(&(end, kind, flags)).is_err() {
                    out.push(format!("a {kind} span at ring {r} ends at ring {end} with no cap"));
                }
            }
        }
        if i.kind() == TREAD {
            let open = (i.words[0] >> 16) & 6;
            if (open & 2 != 0 && !line(r)) || (open & 4 != 0 && !line(r + 1)) {
                out.push(format!("a tread at ring {r} open off the cell line"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::faces::heights::height_at;
    use crate::faces::tests::{cell, input_of, line, mpu, plane, Line};
    use crate::frame::CellKey;

    /// The details and the block of a cell of lines over a terrain of z (metres east and south of `cell()`'s centre).
    fn built(key: CellKey, lines: &[Line], z: &dyn Fn(f64, f64) -> f64) -> (Details, Vec<u32>) {
        let mut input = input_of(key, lines);
        input.terrain = plane(key, z);
        let rules = Rules::embedded();
        let g = crate::graph::build(key, &input, &rules.sections);
        let words = records::build(&g, &input, rules, key, 0, true).near.words;
        (place(&g, &input, rules, &words), words)
    }

    /// A ring's point in frame units from `cell()`'s origin.
    fn global(key: CellKey, r: &Ring) -> V {
        let (o, c) = (key.origin(), cell().origin());
        [(o.x - c.x) as f64 + f64::from(r.p[0]), (o.y - c.y) as f64 + f64::from(r.p[1])]
    }

    /// A sleeper's middle (frame units from `cell()`'s origin): between its seats, or `GAUGE_M` from its seat toward the
    /// ring where its axis leaves the cell.
    fn middle(key: CellKey, d: &Details, i: &Item) -> V {
        let seat = |w: u32| {
            let (r, t) = ((w & 0xffff) as usize, f64::from((w >> 16) & 0xfff) / 4095.0);
            lerp(global(key, &d.rings[r]), global(key, &d.rings[(r + 1).min(d.rings.len() - 1)]), t)
        };
        let a = seat(i.words[0]);
        if i.words[1] >> 28 & AXIS != 0 {
            let x = global(key, &d.rings[(i.words[1] & 0xffff) as usize]);
            let u = unit([x[0] - a[0], x[1] - a[1]]);
            return [a[0] + u[0] * GAUGE_M[0] / mpu(), a[1] + u[1] * GAUGE_M[0] / mpu()];
        }
        lerp(a, seat(i.words[1] & 0xfffffff), 0.5)
    }

    fn of(d: &Details, kind: u32) -> impl Iterator<Item = &Item> {
        d.items.iter().filter(move |i| i.kind() == kind)
    }

    #[test]
    fn two_cells_give_a_track_across_their_line_the_same_rings_there_and_each_sleeper_once() {
        let rail = line("railway", &[("railway", "rail")], &[[-100.0, -100.0], [400.0, 150.0]]);
        let (a, b) = (cell(), CellKey { x: cell().x + 1, y: cell().y });
        let (da, _) = built(a, std::slice::from_ref(&rail), &|_, _| 500.0);
        let (db, _) = built(b, &[rail], &|_, _| 500.0);
        assert!(closure(&da).is_empty() && closure(&db).is_empty(), "{:?}", closure(&da));
        let c = CELL_UNITS as f64;
        let on = |key: CellKey, d: &Details| {
            let mut v: Vec<(i64, i64, u64)> = (d.rings.iter().map(|r| (global(key, r), r.offset)))
                .filter(|(p, _)| p[0] == c)
                .map(|(p, off)| (p[0] as i64, p[1] as i64, off.to_bits()))
                .collect();
            v.sort_unstable();
            v.dedup();
            v
        };
        assert_eq!(on(a, &da).len(), 2, "the two rails and nothing else meet the line");
        assert_eq!(on(a, &da), on(b, &db));
        // Along the track, every sleeper of both cells 0.6 m from the last: none twice, none missing at the line.
        let dir = unit([500.0, 250.0]);
        let mut m: Vec<f64> = (of(&da, SLEEPER).map(|i| middle(a, &da, i)))
            .chain(of(&db, SLEEPER).map(|i| middle(b, &db, i)))
            .map(|p| (p[0] * dir[0] + p[1] * dir[1]) * mpu())
            .collect();
        m.sort_by(f64::total_cmp);
        assert!(m.len() > 400 && of(&da, SLEEPER).any(|i| i.words[1] >> 28 & AXIS != 0));
        for w in m.windows(2) {
            assert!((w[1] - w[0] - SLEEPER_M).abs() < 0.02, "sleepers {} m apart at {}", w[1] - w[0], w[0]);
        }
    }

    #[test]
    fn a_road_across_a_track_sinks_its_rails_continuously_and_lays_no_sleeper_in_it() {
        let rail = line("railway", &[("railway", "rail")], &[[-150.0, 0.0], [150.0, 0.0]]);
        let (c, s) = (libm::cos(PI_3), libm::sin(PI_3));
        let road = line("road", &[("highway", "residential")], &[[-80.0 * c, -80.0 * s], [80.0 * c, 80.0 * s]]);
        let (d, words) = built(cell(), &[rail, road], &|_, _| 500.0);
        assert!(closure(&d).is_empty(), "{:?}", closure(&d));
        let surface = |q: V| find(&words, q).map(|l| records::label(&words, l)[0] & 0xff);
        let crossing = Some(Surface::Crossing as u32);
        let x = |r: usize| (f64::from(d.rings[r].p[0]) - CELL_UNITS as f64 / 2.0) * mpu();
        // A rail head's height over the face under its ring.
        let top = SLEEPER_UP_M + TOP_M[0] * RAIL_SCALE;
        let head = |r: usize| d.rings[r].offset + top;
        let spans: Vec<usize> = of(&d, RAIL).map(|i| i.ring()).filter(|&r| x(r).abs() < 30.0).collect();
        assert!(
            spans.iter().any(|&r| surface(d.rings[r].p.map(f64::from)) == crossing),
            "both rails run through the road"
        );
        for &r in &spans {
            let (dx, dh) = ((x(r + 1) - x(r)).abs(), (head(r + 1) - head(r)).abs());
            assert!(dh <= (top - FLUSH_M) / RAMP_M * dx + 1e-6, "a head steps {dh} m over {dx} m at x {}", x(r));
            if surface(d.rings[r].p.map(f64::from)) == crossing {
                assert!((head(r) - FLUSH_M).abs() < 1e-6, "flush in the crossing at x {}", x(r));
            }
            if x(r).abs() > 15.0 {
                assert!((head(r) - top).abs() < 1e-6, "standing at x {}", x(r));
            }
        }
        assert!(of(&d, CAP).all(|i| x(i.ring()).abs() > 5.0), "no rail ends at the crossing");
        assert!(of(&d, SLEEPER).all(|i| surface(middle(cell(), &d, i)) != crossing));
    }

    #[test]
    fn a_later_track_stands_lower_inside_an_earlier_ones_sleeper_band_at_a_switch() {
        let main = line("railway", &[("railway", "rail")], &[[-150.0, 0.0], [0.0, 0.0], [150.0, 0.0]]);
        let branch = line("railway", &[("railway", "rail")], &[[0.0, 0.0], [150.0, -150.0 / 9.0]]);
        let (d, _) = built(cell(), &[main, branch], &|_, _| 500.0);
        assert!(closure(&d).is_empty(), "{:?}", closure(&d));
        let y = |r: &Ring| (f64::from(r.p[1]) - CELL_UNITS as f64 / 2.0) * mpu();
        let lowered: Vec<&Ring> = d.rings.iter().filter(|r| r.offset == -STACK_M).collect();
        assert!(!lowered.is_empty());
        // The lowered rings are the branch's within the main track's band: none before the switch, none past the band.
        let x = |r: &Ring| (f64::from(r.p[0]) - CELL_UNITS as f64 / 2.0) * mpu();
        assert!(lowered.iter().all(|r| y(r).abs() <= SLEEPER_HALF_M[0] && x(r) > -0.1));
        assert!(d
            .rings
            .iter()
            .filter(|r| y(r).abs() > SLEEPER_HALF_M[0] + 0.01 || x(r) < -0.1)
            .all(|r| r.offset == 0.0));
    }

    #[test]
    fn town_steps_rise_about_16_cm_a_tread_and_two_cells_give_a_tread_on_their_line_one_level() {
        let tags = &[("highway", "steps"), ("surface", "paving_stones")];
        let half = CELL_UNITS as f64 / 2.0 * mpu();
        let mut flight = line("path", tags, &[[half - 7.0, 0.0], [half + 8.0, 0.0]]);
        (flight.z, flight.grade) = (vec![500.0, 503.0], vec![0.2, 0.2]);
        let z = |x: f64, _: f64| 500.0 + 0.2 * (x - half + 7.0);
        let (a, b) = (cell(), CellKey { x: cell().x + 1, y: cell().y });
        let ((da, wa), (db, wb)) = (built(a, &[flight.clone()], &z), built(b, &[flight], &z));
        let levels = |key: CellKey, d: &Details, w: &[u32]| {
            let mut v: Vec<(f64, f64)> = (of(d, TREAD))
                .map(|i| {
                    let r = &d.rings[i.ring()];
                    let h = height_at(w, r.p.map(f64::from), Some(r.label), f64::NAN).h + r.offset;
                    (global(key, r)[0], h)
                })
                .collect();
            v.sort_by(|x, y| x.0.total_cmp(&y.0));
            v
        };
        let (la, lb) = (levels(a, &da, &wa), levels(b, &db, &wb));
        assert!(la.len() > 3 && lb.len() > 3);
        let mut all: Vec<f64> = la.iter().chain(&lb).map(|x| x.1).collect();
        all.sort_by(f64::total_cmp);
        all.dedup_by(|x, y| (*x - *y).abs() < 0.001);
        assert!(all.windows(2).all(|w| (0.15..=0.17).contains(&(w[1] - w[0]))), "{all:?}");
        // The tread on the line is cut in both cells, its ring there one corner at one height.
        let cut = |key: CellKey, d: &Details, w: &[u32]| {
            let r: Vec<&Ring> = d.rings.iter().filter(|r| global(key, r)[0] == CELL_UNITS as f64).collect();
            assert_eq!(r.len(), 1);
            (global(key, r[0]), r[0].offset, height_at(w, r[0].p.map(f64::from), Some(r[0].label), f64::NAN).h)
        };
        let ((pa, oa, ha), (pb, ob, hb)) = (cut(a, &da, &wa), cut(b, &db, &wb));
        assert!(pa == pb && oa == ob && (ha - hb).abs() < 0.001, "{pa:?} {oa} {ha} | {pb:?} {ob} {hb}");
    }

    #[test]
    fn logs_lie_a_height_step_apart_and_no_nearer_than_log_min_m() {
        let at = |len: f64, rise: f64| logs_of(&[(0.0, 1000.0), (len, 1000.0 + rise)]);
        let logs = at(15.0, 3.0);
        assert_eq!(logs.len(), 16);
        assert!(logs.iter().enumerate().all(|(k, l)| (l.0 - k as f64).abs() < 1e-9));
        for (len, rise, gap) in [(10.0, 1.0, 2.0), (6.0, 3.2, 1.125)] {
            let logs = at(len, rise);
            assert!(logs.len() > 2 && logs.windows(2).all(|w| (w[1].0 - w[0].0 - gap).abs() < 1e-9));
        }
        assert_eq!(at(0.5, 0.3).len(), 1, "a short flight lays one");
    }

    const PI_3: f64 = std::f64::consts::FRAC_PI_3;
}
