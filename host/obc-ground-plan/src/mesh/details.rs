//! Storey 0's track and step details near the eye (`place`, in the cell build): a track's rails, sleepers, rack,
//! haul cable and its rollers, and a flight's treads or logs. Each part stands on rings: storey corners with the label
//! that the cell's block finds at their point, so a ring takes the height of the face under it as the cell mesh's
//! corners do. The viewer builds each part from its rings (`plan/details.wgsl`).
//!
//! A track's lines are its rails (its middle ± the rail middle) and, on a rack line or a funicular, its middle. A
//! line's rings are its way's samples and a point each `RING_M` of stroke metres, offset onto the line, and its cuts
//! by the cell lines. A line stands on its bed and on crossing faces; where a ring finds another face (a platform over
//! the rail, the cover behind a portal), the line stops at that face's edge, found to `BISECT_M`. Each run of spans
//! has a cap at each end. A track's top sinks to `FLUSH_M` over its face over `RAMP_M` of stroke metres before a level
//! crossing: where its middle line lies within a ground road's or path's carriageway (half-width and widening about
//! the graph's chord), on its way or on a ground rail way that ends within `RAMP_M` of it at joints and junctions, its
//! metres carried over each node along the least path. That is one value per stroke metre, from the graph alone in global frame units, so all the track's
//! lines take it and every cell gives it alike. A sleeper stands at each `SLEEPER_M` of stroke metres on the spans of
//! both rails, or of one (its other seat past the cell or its rail stopped): then the other seat lies the gauge across
//! at the same height. It sinks with them. A later track (`way_key`, the least of the track's ways about a point)
//! stands `STACK_M` lower inside an earlier one's sleeper band. A flight's span splits into treads by its rise, each a box
//! from under the face to its higher end's graph height, where its rings lie on its own carriageway; or on a fraying
//! look, its logs lie where the curve crosses a height step. Every rule reads only the graph and the cell's block, and
//! each cut is reckoned from the lesser end of its segment, so two cells give a part on their line alike.

use super::edges::{add, at_end, bbox, dist, dot, sub, to_segment, way_key, way_points, Grid};
use crate::faces::records::{self, find, LINE};
use crate::faces::{Rules, Surface};
use crate::frame::{metres_per_unit_at, CELL_UNITS};
use crate::graph::{point as curve_point, s_at, u_at, Family, Level, WayGraph};
use crate::sections::{Finish, Reach, Sections, WaySection, HALF_MAX_M};

/// The kinds of a part (an item's top bits): swept along a line (rail, rack, cable), boxes (sleeper, roller, tread),
/// a swept part's end cap and a log. As `plan/details.wgsl`.
pub const RAIL: u32 = 0;
pub const RACK: u32 = 1;
pub const CABLE: u32 = 2;
pub const SLEEPER: u32 = 3;
pub const ROLLER: u32 = 4;
pub const TREAD: u32 = 5;
pub const CAP: u32 = 6;
pub const LOG: u32 = 7;

/// A ring each `RING_M` of stroke metres between the samples.
const RING_M: f64 = 4.0;
/// A stop face's edge is found to this (metres).
const BISECT_M: f64 = 0.05;
/// A rail's middle from its track's middle, and half a sleeper: standard and narrow gauge (`roads/parts.ts`).
const GAUGE_M: [f64; 2] = [0.7525, 0.53];
const SLEEPER_HALF_M: [f64; 2] = [1.3, 0.9];
/// A sleeper's place each `SLEEPER_M` of stroke metres, a roller's each `ROLLER_M`.
const SLEEPER_M: f64 = 0.6;
const ROLLER_M: f64 = 8.0;
/// A later track stands this much lower inside an earlier one's sleeper band.
const STACK_M: f64 = 0.01;
/// A line's top over its face at a crossing, and the stroke metres over which it rises to standing.
const FLUSH_M: f64 = 0.004;
const RAMP_M: f64 = 3.0;
/// A line's top over its face where it stands: a sleeper's top and a rail, rack or cable (`plan/details.wgsl`).
const SLEEPER_UP_M: f64 = 0.05;
const TOP_M: [f64; 3] = [0.125, 0.09, 0.09];
const RAIL_SCALE: f64 = 1.2;
/// In a sleeper's second word: it has one seat, and the other lies the gauge across its span, to the left with `LEFT`.
const AXIS: u32 = 1;
const LEFT: u32 = 8;
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

    /// Its draw, each with its own corners (`plan/details.ts`): a rail's span, a rack's or a cable's, a box, a cap,
    /// a log.
    pub fn draw(&self) -> usize {
        match self.kind() {
            RAIL => 0,
            RACK | CABLE => 1,
            CAP => 3,
            LOG => 4,
            _ => 2,
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

/// The row of the reach table: how far past the cell square a part reads the graph, a crossing's band a ramp past a
/// ring's span.
pub fn reach(sections: &Sections, graph_m: f64) -> Reach {
    let widen = (0..).map_while(|rank| sections.class_faces(rank)).map(|c| c.widen_m).fold(0.0, f64::max);
    let terms = [
        ("the graph's longest chain", graph_m),
        ("RING_M", RING_M),
        ("RAMP_M", RAMP_M),
        ("HALF_MAX_M + widen_m", HALF_MAX_M + widen),
    ];
    let terms = terms.iter().map(|&(what, m)| (what.to_string(), m)).collect();
    Reach { name: "details".into(), owner: "details", chain: true, terms }
}

type V = [f64; 2];

fn lerp(a: V, b: V, f: f64) -> V {
    add(a, sub(b, a), f)
}

fn cross(a: V, b: V) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

/// The left of a direction in frame axes (y south), seen from above.
fn left(t: V) -> V {
    [t[1], -t[0]]
}

fn rounded(q: V) -> V {
    q.map(libm::round)
}

/// A point of a line: its place, its tangent (unit at the way's points), its stroke metres and the graph's height.
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
        t: lerp(a.t, b.t, f),
        along: a.along + f * (b.along - a.along),
        z: a.z + f * (b.z - a.z),
    }
}

fn in_cell(q: V) -> bool {
    let c = CELL_UNITS as f64;
    (0.0..c).contains(&q[0]) && (0.0..c).contains(&q[1])
}

fn on_square(q: V) -> bool {
    let c = CELL_UNITS as f64;
    (0.0..=c).contains(&q[0]) && (0.0..=c).contains(&q[1])
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
    let mut out: Vec<(At, At)> = (cuts.windows(2))
        .filter(|w| w[1].0 > w[0].0 && w[0].1.p != w[1].1.p && on_square(lerp(w[0].1.p, w[1].1.p, 0.5)))
        .map(|w| (w[0].1, w[1].1))
        .collect();
    if swap {
        out.reverse();
        out.iter_mut().for_each(|x| *x = (x.1, x.0));
    }
    out
}

/// The share of segment p + s·d (s from 0 to 1) within r of segment a → b, if any: the hull of its shares in the band
/// beside a → b and in the discs at its ends, one interval as the capsule is convex.
fn capsule(p: V, d: V, a: V, b: V, r: f64) -> Option<(f64, f64)> {
    let mut out = (f64::INFINITY, f64::NEG_INFINITY);
    let mut take = |lo: f64, hi: f64| {
        let (lo, hi) = (lo.max(0.0), hi.min(1.0));
        if lo <= hi {
            out = (out.0.min(lo), out.1.max(hi));
        }
    };
    let dd = dot(d, d);
    for e in [a, b] {
        let w = sub(p, e);
        let (h, c) = (dot(w, d), dot(w, w) - r * r);
        if dd == 0.0 {
            if c <= 0.0 {
                take(0.0, 1.0);
            }
        } else if h * h - dd * c >= 0.0 {
            let root = libm::sqrt(h * h - dd * c);
            take((-h - root) / dd, (-h + root) / dd);
        }
    }
    let (u, w) = (sub(b, a), sub(p, a));
    let uu = dot(u, u);
    if uu > 0.0 {
        let l = libm::sqrt(uu);
        // Along a → b from 0 to 1 and across within r: each linear in s.
        let mut band = (0.0_f64, 1.0_f64);
        for (v0, v1, lo, hi) in [(dot(w, u) / uu, dot(d, u) / uu, 0.0, 1.0), (cross(u, w) / l, cross(u, d) / l, -r, r)]
        {
            if v1 == 0.0 {
                if !(lo..=hi).contains(&v0) {
                    band = (1.0, 0.0);
                }
            } else {
                let (x, y) = ((lo - v0) / v1, (hi - v0) / v1);
                band = (band.0.max(x.min(y)), band.1.min(x.max(y)));
            }
        }
        take(band.0, band.1);
    }
    (out.0 <= out.1).then_some(out)
}

/// The stroke metres from m to the nearest of the crossing intervals.
fn crossing_m(zones: &[(f64, f64)], m: f64) -> f64 {
    zones.iter().map(|&(lo, hi)| (lo - m).max(m - hi).max(0.0)).fold(f64::INFINITY, f64::min)
}

/// A station of a line in the cell: its point, the label there, whether the line stands on its face, and whether the
/// line leaves the cell square after it.
#[derive(Clone, Copy, Debug)]
struct Station {
    at: At,
    label: u32,
    stands: bool,
    gap: bool,
}

/// A span of a line: its first ring in the table (the next ring is its other end) and their stroke metres.
#[derive(Clone, Copy, Debug)]
struct Span {
    ring: u32,
    along: [f64; 2],
}

impl Span {
    fn lo(&self) -> f64 {
        self.along[0].min(self.along[1])
    }
}

/// The seat at stroke metres m on the span of a line that holds it from its least end on (the spans by their least
/// stroke metres): its ring and its share in 12 bits, rounded from that end, so either direction gives one point.
fn seat(spans: &[Span], m: f64) -> Option<u32> {
    let s = spans[..spans.partition_point(|s| s.lo() <= m)].last()?;
    let hi = s.along[0].max(s.along[1]);
    if hi <= m {
        return None;
    }
    let k = libm::round((m - s.lo()) / (hi - s.lo()) * 4095.0) as u32;
    let k = if s.along[0] <= s.along[1] { k } else { 4095 - k };
    Some(s.ring | k << 16)
}

/// A ground road's or path's chord in global frame units and its band (metres).
type Chord = ([V; 2], f64);
/// A ground track's chord, its sleeper half (metres), its way's place in `way_key` order and its way.
type Bed = ([V; 2], f64, u32, u32);
/// A track way's place in `way_key` order and the way.
type Key = (u32, u32);

struct Cx<'a> {
    g: &'a WayGraph,
    words: &'a [u32],
    origin: V,
    out: Details,
    tracks: (Grid, Vec<Bed>),
    /// The ground roads' and paths' chords and the widest band.
    roads: (Grid, Vec<Chord>, f64),
    /// Per way, at its start and its end, the node there (a joint or junction where it ends) and its stroke metres;
    /// per node the rail ways that end there, each with that end.
    ends: Vec<[(Option<u32>, f64); 2]>,
    nodes: Vec<Vec<(u32, usize)>>,
    /// Per way the ways its track runs on into: at a joint, and at a junction's two arms that are each other's most
    /// nearly opposite (not a branch).
    own: Vec<Vec<u32>>,
}

impl Cx<'_> {
    fn mpu(&self, q: V) -> f64 {
        metres_per_unit_at(self.origin[1] + q[1])
    }

    /// The node of a point: its level-18 square of the cell, row by row.
    fn node(q: V) -> u8 {
        let s = CELL_UNITS as f64 / 4.0;
        let k = |v: f64| (libm::floor(v / s) as i32).clamp(0, 3) as u8;
        k(q[1]) * 4 + k(q[0])
    }

    fn station(&self, at: At) -> Station {
        let (stands, label) = match find(self.words, at.p) {
            Some(l) => {
                let s = records::label(self.words, l)[0] & 0xff;
                (s == Surface::RailBed as u32 || s == Surface::Crossing as u32, l)
            }
            None => (false, super::NO_LABEL),
        };
        Station { at, label, stands, gap: false }
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

    /// Whether q lies in the sleeper band of another ground track earlier in `way_key` order than way w's track at q:
    /// the least of w and the ways its track runs on into whose bands hold q, so two ways of a track agree at their
    /// joint.
    fn lowered(&self, q: V, (key, w): Key) -> bool {
        let (grid, chords) = &self.tracks;
        let mpu = self.mpu(q);
        let (mut mine, mut other) = (key, u32::MAX);
        grid.any(q, SLEEPER_HALF_M[0] / mpu, |k| {
            let ([a, b], half, place, v) = chords[k as usize];
            if to_segment(q, a, b) * mpu <= half {
                if v == w || self.own[w as usize].contains(&v) {
                    mine = mine.min(place);
                } else {
                    other = other.min(place);
                }
            }
            false
        });
        other < mine
    }

    /// The stroke metres, in way w's own, where the middle line of its track lies within a crossing band, each with
    /// the box of its chord (global frame units) where `near` keeps it: on w's chords and on those of the rail ways
    /// that end within `RAMP_M` of w's ends, through every rail end at each node, each by its least path in metres,
    /// so every end at a node sees one distance to each zone. A way's metres map into w's along that path.
    fn zones(&self, w: u32, near: impl Fn(&[f64; 4]) -> bool) -> Vec<(f64, f64, [f64; 4])> {
        let (grid, chords, widest) = &self.roads;
        let mpu = metres_per_unit_at(self.origin[1] + CELL_UNITS as f64 / 2.0);
        let mut ways = vec![(w, 1.0, 0.0)];
        let mut seen = vec![false; self.g.ways.len()];
        seen[w as usize] = true;
        // Nodes to pass, each with its path metres from w, its metres in w's and the way w's metres run out there.
        let e = &self.ends[w as usize];
        let mut open: Vec<(u32, f64, f64, f64)> = (0..2)
            .filter_map(|k| e[k].0.map(|n| (n, 0.0, e[k].1, if e[k].1 >= e[1 - k].1 { 1.0 } else { -1.0 })))
            .collect();
        let mut passed = vec![false; self.nodes.len()];
        while let Some(i) = (0..open.len()).min_by(|&a, &b| open[a].1.total_cmp(&open[b].1)) {
            let (n, d, m, out) = open.swap_remove(i);
            if std::mem::replace(&mut passed[n as usize], true) {
                continue;
            }
            for &(x, k) in &self.nodes[n as usize] {
                if std::mem::replace(&mut seen[x as usize], true) {
                    continue;
                }
                let [near_end, far_end] = [self.ends[x as usize][k], self.ends[x as usize][1 - k]];
                let sx = if far_end.1 >= near_end.1 { out } else { -out };
                let tx = m - sx * near_end.1;
                ways.push((x, sx, tx));
                let past = d + (far_end.1 - near_end.1).abs();
                if let Some(next) = far_end.0.filter(|_| past < RAMP_M) {
                    open.push((next, past, sx * far_end.1 + tx, out));
                }
            }
        }
        let mut out = Vec::new();
        for (v, sv, tv) in ways {
            let way = &self.g.ways[v as usize];
            if way.level != Level::Ground {
                continue;
            }
            let samples = &self.g.samples[way.samples.start as usize..way.samples.end as usize];
            for s in samples.windows(2).filter(|s| s[1].span <= s[0].span + 1) {
                let [p, q] = [&s[0], &s[1]].map(|x| [0, 1].map(|k| self.origin[k] + f64::from(x.pos[k])));
                let b = bbox(&[p, q]);
                if !near(&b) {
                    continue;
                }
                let d = sub(q, p);
                let reach = libm::hypot(d[0], d[1]) / 2.0 + (widest + 1.0) / mpu;
                grid.any(lerp(p, q, 0.5), reach, |k| {
                    let ([a, c], band) = chords[k as usize];
                    if let Some((x, y)) = capsule(p, d, a, c, band / metres_per_unit_at(a[1])) {
                        let m = |f: f64| sv * (s[0].along_m + f * (s[1].along_m - s[0].along_m)) + tv;
                        out.push((m(x).min(m(y)), m(x).max(m(y)), b));
                    }
                    false
                });
            }
        }
        out
    }

    /// A ring's offset: lower inside an earlier track's sleeper band, and sunk from a line's top over its face
    /// `top_m` toward `FLUSH_M` by the stroke metres to a crossing.
    fn offset(&self, at: &At, key: Key, top_m: f64, zones: &[(f64, f64)]) -> f64 {
        let ramp = (crossing_m(zones, at.along) / RAMP_M).min(1.0);
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

    /// The two stations about the edge of a stop face between a and b, one standing and one not: found to
    /// `BISECT_M` at rounded points, reckoned from the lesser end.
    fn edge(&self, a: &Station, b: &Station) -> [Station; 2] {
        let (mut p, mut r, swap) = if a.at.p < b.at.p { (*a, *b, false) } else { (*b, *a, true) };
        let (from, to) = (p.at, r.at);
        let len = dist(from.p, to.p) * self.mpu(from.p);
        let (mut lo, mut hi) = (0.0, 1.0);
        while (hi - lo) * len > BISECT_M {
            let mut x = mix(&from, &to, (lo + hi) / 2.0);
            x.p = rounded(x.p);
            let m = self.station(x);
            if m.stands == p.stands {
                (lo, p) = ((lo + hi) / 2.0, m);
            } else {
                (hi, r) = ((lo + hi) / 2.0, m);
            }
        }
        if swap {
            [r, p]
        } else {
            [p, r]
        }
    }

    /// The runs of a line (its points in order): per run its rings, a span item between each two and a cap at each
    /// end. Returns its spans.
    fn line(&mut self, pts: &[At], kind: u32, flags: u32, code: u32, key: Key, zones: &[(f64, f64)]) -> Vec<Span> {
        let mut st: Vec<Station> = Vec::new();
        for w in pts.windows(2) {
            for (a, b) in pieces(&w[0], &w[1]) {
                if st.last().map(|s| s.at.p) != Some(a.p) {
                    if let Some(last) = st.last_mut() {
                        last.gap = true;
                    }
                    st.push(self.station(a));
                }
                let (from, to) = (*st.last().unwrap(), self.station(b));
                if from.stands != to.stands {
                    let edge = self.edge(&from, &to);
                    st.extend(edge.into_iter().filter(|s| s.at.p != from.at.p && s.at.p != to.at.p));
                }
                st.push(to);
            }
        }
        let stands = |i: usize| st[i].stands && st[i + 1].stands && !st[i].gap;
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
                let offset = self.offset(&s.at, key, top, zones);
                self.ring(&s.at, s.label, offset);
            }
            for k in i..j {
                let (ring, a, b) = (first + (k - i) as u32, st[k].at, st[k + 1].at);
                self.item(lerp(a.p, b.p, 0.5), [word(kind, ring, flags), code]);
                spans.push(Span { ring, along: [a.along, b.along] });
            }
            for (end, k, r) in [(0, i, first), (1, j, first + (j - i) as u32)] {
                self.item(st[k].at.p, [word(CAP, r, flags | end << 3 | kind << 4), code]);
            }
            i = j;
        }
        spans
    }

    /// A track's lines, sleepers and rollers, by its section's finish and class.
    fn track(&mut self, s: &WaySection, finish: Finish, key: Key) {
        let w = key.1;
        let narrow = matches!(finish, Finish::Narrow | Finish::NarrowRack);
        let code = u32::from(finish.code());
        let mut pts = Vec::new();
        way_points(self.g, w, 1, RING_M, &mut pts);
        // The zones on chords that come within a ramp, a gauge and a ring of the cell square.
        let (c, mpu) = (CELL_UNITS as f64, metres_per_unit_at(self.origin[1] + CELL_UNITS as f64 / 2.0));
        let r = (RAMP_M + GAUGE_M[0] + RING_M) / mpu;
        let [x, y] = self.origin;
        let meets = |b: &[f64; 4]| b[2] >= x - r && b[0] <= x + c + r && b[3] >= y - r && b[1] <= y + c + r;
        let zones: Vec<(f64, f64)> = self.zones(w, meets).into_iter().map(|z| (z.0, z.1)).collect();
        // The way's points and a point where the ramp bends: at each zone's ends, a ramp past them, and midway
        // between two zones nearer than two ramps; so a line takes the ramp exactly and a ring on the crossing face.
        // Each by its span, in the order the points read. A bend within a centimetre past another bend or of a way's
        // point gives way, which no reading order changes.
        let mut at: Vec<(At, u32)> =
            pts.iter().map(|p| (At { p: p.pos, t: p.t, along: p.along, z: p.z }, p.span)).collect();
        let ahead = pts.len() > 1 && pts[1].along > pts[0].along;
        let turn = if ahead != self.g.parts[self.g.ways[w as usize].part as usize].backward { 1.0 } else { -1.0 };
        let mut bends: Vec<f64> = zones.iter().flat_map(|&(lo, hi)| [lo - RAMP_M, lo, hi, hi + RAMP_M]).collect();
        for (a, b) in zones.iter().flat_map(|a| zones.iter().map(move |b| (a, b))) {
            if b.0 > a.1 && b.0 - a.1 < 2.0 * RAMP_M {
                bends.push((a.1 + b.0) / 2.0);
            }
        }
        let near = |a: f64, b: f64| (a - b).abs() < 0.01;
        for &m in &bends {
            if bends.iter().any(|&b| b < m && near(b, m)) || pts.iter().any(|p| near(p.along, m)) {
                continue;
            }
            if let Some((k, s)) = span_at(self.g, w, m) {
                let a = curve_at(self.g, w, k, s);
                let i = at.partition_point(|x| (x.0.along < m) == ahead);
                at.insert(i, (At { t: a.t.map(|v| v * turn), ..a }, k));
            }
        }
        let g = GAUGE_M[usize::from(narrow)];
        let n = u32::from(narrow) << 1;
        let mut rails = [Vec::new(), Vec::new()];
        let mut middle = Vec::new();
        // Each run of points with no jump of the graph between them.
        for run in at.chunk_by(|a, b| a.1.abs_diff(b.1) <= 1) {
            let line = |off: f64, cx: &Cx| -> Vec<At> { run.iter().map(|p| cx.offset_point(&p.0, off)).collect() };
            let (l, r, m) = (line(g, self), line(-g, self), line(0.0, self));
            rails[0].extend(self.line(&l, RAIL, n | 1, code, key, &zones));
            rails[1].extend(self.line(&r, RAIL, n, code, key, &zones));
            if matches!(finish, Finish::Rack | Finish::NarrowRack) || s.class == "rack" {
                self.line(&m, RACK, n, code, key, &zones);
            }
            if s.class == "funicular" {
                middle.extend(self.line(&m, CABLE, n, code, key, &zones));
            }
        }
        rails.iter_mut().chain([&mut middle]).for_each(|x| x.sort_by(|a, b| a.lo().total_cmp(&b.lo())));
        let flags = n | u32::from(matches!(s.class, "funicular" | "rack")) << 2;
        for (m, q) in marks(self.g, w, SLEEPER_M).into_iter().filter(|x| in_cell(x.1)) {
            let words = match rails.each_ref().map(|r| seat(r, m)) {
                [Some(a), Some(b)] => Some([a | SLEEPER << 28, b | flags << 28]),
                // A seat alone (the other past the cell, or its rail stopped under a platform): the other lies the
                // gauge across its span, to its left or right, at its height.
                [Some(a), None] | [None, Some(a)] => {
                    let r = &self.out.rings[(a & 0xffff) as usize..];
                    let [p, v] = [r[0].p, r[1].p].map(|x| x.map(f64::from));
                    let near = lerp(p, v, f64::from(a >> 16) / 4095.0);
                    let side = if dot(sub(q, near), left(sub(v, p))) > 0.0 { LEFT } else { 0 };
                    Some([a | SLEEPER << 28, (AXIS | side | flags) << 28])
                }
                _ => None,
            };
            if let Some(words) = words {
                self.item(q, words);
            }
        }
        if s.class == "funicular" {
            for (m, q) in marks(self.g, w, ROLLER_M).into_iter().filter(|x| in_cell(x.1)) {
                if let Some(a) = seat(&middle, m) {
                    self.item(q, [a | ROLLER << 28, code]);
                }
            }
        }
    }

    /// A point of a way offset `off_m` to its left: the way's point and the offset each rounded, so two cells give it
    /// alike (a post point between samples is the curve's, not a whole unit).
    fn offset_point(&self, p: &At, off_m: f64) -> At {
        let (n, k) = (left(p.t), off_m / self.mpu(p.p));
        At { p: [0, 1].map(|i| libm::round(p.p[i]) + libm::round(n[i] * k)), ..*p }
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
                    self.item(lerp(pa.p, pb.p, 0.5), [word(TREAD, first, 0), (half as f32).to_bits()]);
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
                let seed = level as u64 ^ u64::from(way.id) << 40 ^ LOG_SEED;
                let random = |salt: u64| (splitmix64(seed ^ salt << 32) >> 11) as f64 / (1u64 << 53) as f64;
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

/// Per span of way w that the graph holds: its index and the way's span metres in it.
fn spans_of(g: &WayGraph, w: u32) -> Vec<(u32, f64, f64)> {
    let way = &g.ways[w as usize];
    let mut ks: Vec<u32> =
        g.samples[way.samples.start as usize..way.samples.end as usize].iter().map(|s| s.span).collect();
    ks.dedup();
    let range = |k: u32| {
        let span = &g.parts[way.part as usize].spans[k as usize];
        let s0 = if k == way.from.span { s_at(span, way.from.u) } else { 0.0 };
        let s1 = if k == way.to.span { s_at(span, way.to.u) } else { span.len_m };
        (k >= way.from.span && k <= way.to.span && s1 > s0).then_some((k, s0, s1))
    };
    ks.into_iter().filter_map(range).collect()
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

/// The span of way w and its metres there at stroke metres m, where the graph holds it.
fn span_at(g: &WayGraph, w: u32, m: f64) -> Option<(u32, f64)> {
    let part = &g.parts[g.ways[w as usize].part as usize];
    let sign = if part.backward { -1.0 } else { 1.0 };
    let at = |k: u32| (m - part.spans[k as usize].along0_m) * sign;
    spans_of(g, w).into_iter().find(|&(k, s0, s1)| (s0..=s1).contains(&at(k))).map(|(k, ..)| (k, at(k)))
}

/// The stroke metres of way w at its station.
fn along_at(g: &WayGraph, w: u32, at: &crate::graph::Station) -> f64 {
    let part = &g.parts[g.ways[w as usize].part as usize];
    let span = &part.spans[at.span as usize];
    span.along0_m + if part.backward { -1.0 } else { 1.0 } * s_at(span, at.u)
}

/// The marks of way w at each `step` of its stroke metres: per mark its metres and its point on the curve.
fn marks(g: &WayGraph, w: u32, step: f64) -> Vec<(f64, V)> {
    let way = &g.ways[w as usize];
    let part = &g.parts[way.part as usize];
    let sign = if part.backward { -1.0 } else { 1.0 };
    let mut out = Vec::new();
    for (k, s0, s1) in spans_of(g, w) {
        let a0 = part.spans[k as usize].along0_m;
        let (lo, hi) = (a0 + sign * s0, a0 + sign * s1);
        for j in libm::ceil(lo.min(hi) / step) as i64..=libm::floor(lo.max(hi) / step) as i64 {
            let s = (j as f64 * step - a0) * sign;
            // A mark on a span's end belongs to the span after it on the way.
            if (s0..s1).contains(&s) || (s == s1 && k == way.to.span) {
                out.push((j as f64 * step, curve_at(g, w, k, s).p));
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
    let samples = |w: usize| &g.samples[g.ways[w].samples.start as usize..g.ways[w].samples.end as usize];
    let mut beds = Vec::new();
    for &w in &tracks {
        let narrow = matches!(section(w).finish, Some(Finish::Narrow | Finish::NarrowRack));
        for s in samples(w).windows(2).filter(|s| s[1].span <= s[0].span + 1) {
            let ends = [s[0].pos, s[1].pos].map(|p| p.map(f64::from));
            beds.push((ends, SLEEPER_HALF_M[usize::from(narrow)], key[w], w as u32));
        }
    }
    let bed_boxes: Vec<[f64; 4]> = beds.iter().map(|c| bbox(&c.0)).collect();
    let o = g.cell.origin();
    let origin = [o.x as f64, o.y as f64];
    let mut roads: Vec<Chord> = Vec::new();
    for w in (0..g.ways.len()).filter(|&w| ground(w, Family::Road) || ground(w, Family::Path)) {
        let half = section(w).half_m;
        for p in samples(w).windows(2).filter(|p| p[1].span <= p[0].span + 1) {
            let ends = [p[0].pos, p[1].pos].map(|q| [0, 1].map(|k| origin[k] + f64::from(q[k])));
            roads.push((ends, half + p[0].widen_m.max(p[1].widen_m)));
        }
    }
    let widest = roads.iter().map(|r| r.1).fold(0.0, f64::max);
    let road_boxes: Vec<[f64; 4]> = roads.iter().map(|c| bbox(&c.0)).collect();
    // The rail ways' ends at the joints and junctions; a track runs on at a joint and at a junction's two arms that
    // are each other's most nearly opposite (as `edges` chains roads).
    let mut ends: Vec<[(Option<u32>, f64); 2]> = (0..g.ways.len() as u32)
        .map(|w| [&g.ways[w as usize].from, &g.ways[w as usize].to].map(|s| (None, along_at(g, w, s))))
        .collect();
    let (mut nodes, mut own) = (Vec::new(), vec![Vec::new(); g.ways.len()]);
    for arms in (g.joints.iter().map(|j| &j.arms[..])).chain(g.junctions.iter().map(|j| &j.arms[..])) {
        let rail = |k: usize| g.ways[arms[k].way as usize].family == Family::Rail && at_end(g, &arms[k]);
        let cos = |a: usize, b: usize| libm::cos(arms[a].angle - arms[b].angle);
        let opposite = |k: usize| (0..arms.len()).filter(|&c| c != k).min_by(|&a, &b| cos(a, k).total_cmp(&cos(b, k)));
        for k in (0..arms.len()).filter(|&k| rail(k)) {
            ends[arms[k].way as usize][usize::from(arms[k].dir < 0)].0 = Some(nodes.len() as u32);
            if let Some(o) = opposite(k).filter(|&o| opposite(o) == Some(k) && rail(o) && arms[o].way != arms[k].way) {
                own[arms[k].way as usize].push(arms[o].way);
            }
        }
        nodes.push((0..arms.len()).filter(|&k| rail(k)).map(|k| (arms[k].way, usize::from(arms[k].dir < 0))).collect());
    }
    let mut cx = Cx {
        g,
        words,
        origin,
        out: Details::default(),
        tracks: (Grid::of(8192.0, &bed_boxes), beds),
        roads: (Grid::of(8192.0, &road_boxes), roads, widest),
        ends,
        nodes,
        own,
    };
    for w in tracks {
        let s = section(w);
        if let Some(finish) = s.finish {
            cx.track(&s, finish, (key[w], w as u32));
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::faces::heights::height_at;
    use crate::faces::tests::{cell, input_of, line, mpu, plane, Line};
    use crate::frame::CellKey;

    fn unit(v: V) -> V {
        let l = libm::hypot(v[0], v[1]);
        [v[0] / l, v[1] / l]
    }

    /// The graph, the block and the details of a cell of lines over a terrain of z (metres east and south of `cell()`'s
    /// centre).
    fn built(key: CellKey, lines: &[Line], z: &dyn Fn(f64, f64) -> f64) -> (WayGraph, Vec<u32>, Details) {
        let mut input = input_of(key, lines);
        input.terrain = plane(key, z);
        let rules = Rules::embedded();
        let g = crate::graph::build(key, &input, &rules.sections);
        let words = records::build(&g, &input, rules, key, 0, true).near.words;
        let d = place(&g, &input, rules, &words);
        (g, words, d)
    }

    /// A ring's point in frame units from `cell()`'s origin.
    fn global(key: CellKey, r: &Ring) -> V {
        let (o, c) = (key.origin(), cell().origin());
        [(o.x - c.x) as f64 + f64::from(r.p[0]), (o.y - c.y) as f64 + f64::from(r.p[1])]
    }

    /// A seat's point (frame units from `cell()`'s origin) and offset.
    fn seat_at(key: CellKey, d: &Details, w: u32) -> (V, f64) {
        let (r, t) = ((w & 0xffff) as usize, f64::from((w >> 16) & 0xfff) / 4095.0);
        let (a, b) = (&d.rings[r], &d.rings[r + 1]);
        (lerp(global(key, a), global(key, b), t), a.offset + t * (b.offset - a.offset))
    }

    /// A sleeper's middle (frame units from `cell()`'s origin) and its seats' mean offset.
    fn middle(key: CellKey, d: &Details, i: &Item) -> (V, f64) {
        let (a, off) = seat_at(key, d, i.words[0]);
        let flags = i.words[1] >> 28;
        if flags & AXIS != 0 {
            let dir = unit(sub(global(key, &d.rings[i.ring() + 1]), global(key, &d.rings[i.ring()])));
            let side = if flags & LEFT != 0 { 1.0 } else { -1.0 };
            return (add(a, left(dir), side * GAUGE_M[0] / mpu()), off);
        }
        let (b, off_b) = seat_at(key, d, i.words[1] & 0x0fff_ffff);
        (lerp(a, b, 0.5), (off + off_b) / 2.0)
    }

    fn of(d: &Details, kind: u32) -> impl Iterator<Item = &Item> {
        d.items.iter().filter(move |i| i.kind() == kind)
    }

    #[test]
    fn two_cells_and_either_direction_give_a_track_across_their_line_the_same_rings_and_each_sleeper_once() {
        let rail = |pts: &[[f64; 2]], z: [f64; 2], grade: f64| {
            let mut l = line("railway", &[("railway", "rail")], pts);
            (l.z, l.grade) = (z.to_vec(), vec![grade; 2]);
            l
        };
        let len = libm::hypot(500.0, 250.0);
        let forward = rail(&[[-100.0, -100.0], [400.0, 150.0]], [500.0, 505.0], 5.0 / len);
        let (a, b) = (cell(), CellKey { x: cell().x + 1, y: cell().y });
        let flat = |_: f64, _: f64| 500.0;
        let (ga, _, da) = built(a, std::slice::from_ref(&forward), &flat);
        let (_, _, db) = built(b, std::slice::from_ref(&forward), &flat);
        // The same way drawn the other way round, its stroke metres running back from its far end.
        let span = ga.parts[0].spans.last().unwrap();
        let mut back = rail(&[[400.0, 150.0], [-100.0, -100.0]], [505.0, 500.0], -5.0 / len);
        back.plan = (span.along0_m + span.len_m, true);
        let (_, _, dr) = built(a, &[back], &flat);
        let c = CELL_UNITS as f64;
        let rings = |key: CellKey, d: &Details, x: Option<f64>| {
            let mut v: Vec<(i64, i64, u64)> = (d.rings.iter().map(|r| (global(key, r), r.offset)))
                .filter(|(p, _)| x.is_none_or(|x| p[0] == x))
                .map(|(p, off)| (p[0] as i64, p[1] as i64, off.to_bits()))
                .collect();
            v.sort_unstable();
            v.dedup();
            v
        };
        assert_eq!(rings(a, &da, Some(c)).len(), 2, "the two rails and nothing else meet the line");
        assert_eq!(rings(a, &da, Some(c)), rings(b, &db, Some(c)));
        assert_eq!(rings(a, &da, None), rings(a, &dr, None), "a reversed way gives the same rings");
        // Along the track, every sleeper of both cells 0.6 m from the last: none twice, none missing at the line.
        let dir = unit([500.0, 250.0]);
        let along = |key: CellKey, d: &Details| -> Vec<f64> {
            let mut m: Vec<f64> = of(d, SLEEPER).map(|i| dot(middle(key, d, i).0, dir) * mpu()).collect();
            m.sort_by(f64::total_cmp);
            m
        };
        let mut m: Vec<f64> = along(a, &da).into_iter().chain(along(b, &db)).collect();
        m.sort_by(f64::total_cmp);
        assert!(m.len() > 400 && of(&da, SLEEPER).any(|i| i.words[1] >> 28 & AXIS != 0));
        for w in m.windows(2) {
            assert!((w[1] - w[0] - SLEEPER_M).abs() < 0.02, "sleepers {} m apart at {}", w[1] - w[0], w[0]);
        }
        let (f, r) = (along(a, &da), along(a, &dr));
        assert!(f.len() == r.len() && f.iter().zip(&r).all(|(x, y)| (x - y).abs() < 0.01), "reversed sleepers");
    }

    #[test]
    fn a_road_across_a_track_sinks_the_whole_track_continuously_and_buries_its_sleepers() {
        // Two ways of the track meet in the ramp, their stroke metres 5 cm apart there.
        let rail = line("railway", &[("railway", "rail")], &[[-150.0, 0.0], [4.0, 0.0]]);
        let mut on = line("railway", &[("railway", "rail")], &[[4.0, 0.0], [150.0, 0.0]]);
        on.plan = (154.05, false);
        let (c, s) = (libm::cos(PI_3), libm::sin(PI_3));
        let road = line("road", &[("highway", "residential")], &[[-80.0 * c, -80.0 * s], [80.0 * c, 80.0 * s]]);
        // A road beside the track with its edge over the bed, as the face plan's test: no crossing.
        let beside = line("road", &[("highway", "residential")], &[[20.0, 3.5], [150.0, 3.5]]);
        let (_, words, d) = built(cell(), &[rail, on, road, beside], &|_, _| 500.0);
        let surface = |q: V| find(&words, q).map(|l| records::label(&words, l)[0] & 0xff);
        let crossing = Some(Surface::Crossing as u32);
        let x = |r: usize| (f64::from(d.rings[r].p[0]) - CELL_UNITS as f64 / 2.0) * mpu();
        // A rail head's height over the face under its ring.
        let top = SLEEPER_UP_M + TOP_M[0] * RAIL_SCALE;
        let head = |r: usize| d.rings[r].offset + top;
        let spans: Vec<usize> = of(&d, RAIL).map(|i| i.ring()).filter(|&r| x(r).abs() < 30.0).collect();
        assert!(spans.iter().any(|&r| surface(d.rings[r].p.map(f64::from)) == crossing), "the rails run through");
        assert!(spans.iter().any(|&r| (head(r) - FLUSH_M).abs() < 1e-9), "flush in the road");
        for &r in &spans {
            let (dm, dh) = ((d.rings[r + 1].along - d.rings[r].along).abs(), (head(r + 1) - head(r)).abs());
            assert!(dh <= (top - FLUSH_M) / RAMP_M * dm + 1e-9, "a head steps {dh} m over {dm} m at x {}", x(r));
            if x(r).abs() > 15.0 {
                assert!((head(r) - top).abs() < 1e-9, "standing at x {}", x(r));
            }
        }
        // One track: both rails' heads alike at each stroke metre.
        let mut heads: Vec<(u64, u64)> = d.rings.iter().map(|r| (r.along.to_bits(), r.offset.to_bits())).collect();
        heads.sort_unstable();
        heads.dedup();
        assert!(heads.windows(2).all(|w| w[0].0 != w[1].0), "a twisted track");
        // No step where the two ways meet, and no other rail end near the crossing.
        let mut ends: Vec<([i32; 2], u64)> =
            of(&d, CAP).map(|i| (d.rings[i.ring()].p, d.rings[i.ring()].offset.to_bits())).collect();
        ends.sort_unstable();
        ends.dedup();
        assert!(ends.windows(2).all(|w| w[0].0 != w[1].0), "a step at the joint");
        assert!(of(&d, CAP).all(|i| x(i.ring()).abs() > 100.0 || (x(i.ring()) - 4.0).abs() < 0.01), "a rail end");
        for i in of(&d, SLEEPER) {
            let (q, off) = middle(cell(), &d, i);
            assert!(surface(q) != crossing || off + SLEEPER_UP_M < 0.0, "a sleeper's top shows in the road at {q:?}");
        }
    }

    #[test]
    fn a_later_track_stands_lower_inside_an_earlier_ones_sleeper_band_at_a_switch() {
        let main = line("railway", &[("railway", "rail")], &[[-150.0, 0.0], [0.0, 0.0], [150.0, 0.0]]);
        let branch = line("railway", &[("railway", "rail")], &[[0.0, 0.0], [150.0, -150.0 / 9.0]]);
        let (_, _, d) = built(cell(), &[main, branch], &|_, _| 500.0);
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
        let ((_, wa, da), (_, wb, db)) = (built(a, &[flight.clone()], &z), built(b, &[flight], &z));
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
