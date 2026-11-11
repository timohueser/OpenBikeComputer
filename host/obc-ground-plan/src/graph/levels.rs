//! The levels of bridges and tunnels, after `build`: per crossing the room between its two ways and
//! its step of the ladder; per station of a tagged way its state; the end lines where a state changes.
//! Nothing on the device calls it yet: the faces still read `Way::level` and `Way::deck`.
//!
//! Every decision reads within the row of `Structures::reach`: a crossing's corners along each way up
//! to `PLACE_M`; a station's tag end along its way up to `open_max_m`, an A line up to `A_MAX_M`; the
//! bodies and strokes within `GRID_M` of a station.

use super::curve::point;
use super::parts::{classify, Rule};
use super::points::{advance, between, exact, key, Piece};
use super::{CrossingKind, Family, Fault, Level, Sample, Station, Way, WayGraph, PLACE_M};
use crate::frame::{metres_per_unit_at, GlobalPoint};
use crate::sections::{leading_number, Reach, Sections, HALF_MAX_M};
use crate::CellInput;

/// The rule values of the structures: `plan.json` `structures`.
#[derive(Clone, Debug, PartialEq)]
pub struct Structures {
    /// The cover over a full bore.
    pub cover_m: f64,
    /// Free ground over a bore may lack this much of the least room.
    pub lift_max_m: f64,
    /// A tunnel way is open only this far along from a tag end.
    pub open_max_m: f64,
    /// A deck shorter than this between its end lines is a ground way.
    pub deck_min_m: f64,
    /// The band past a deck's paved edge.
    pub edge_band_m: f64,
    /// A closed end gets a door from this step on.
    pub door_min_m: f64,
}

impl Structures {
    pub const FIELDS: [&'static str; 6] =
        ["cover_m", "lift_max_m", "open_max_m", "deck_min_m", "edge_band_m", "door_min_m"];

    pub fn new([cover_m, lift_max_m, open_max_m, deck_min_m, edge_band_m, door_min_m]: [f64; 6]) -> Structures {
        Structures { cover_m, lift_max_m, open_max_m, deck_min_m, edge_band_m, door_min_m }
    }

    /// The row of the reach table: the graph's own longest chain, then a station's read along its way,
    /// the reads about it, and a crossing's corners.
    pub fn reach(&self, graph_m: f64) -> Reach {
        let terms = [
            ("the graph's longest chain", graph_m),
            ("along the way: max(open_max_m, A_MAX_M)", self.open_max_m.max(A_MAX_M)),
            ("about a station: GRID_M", GRID_M),
            ("a crossing's corners along each way: PLACE_M", PLACE_M),
        ];
        Reach {
            name: "graph levels".into(),
            owner: "graph",
            chain: true,
            terms: terms.iter().map(|&(what, m)| (what.to_string(), m)).collect(),
        }
    }
}

/// The ladder: what the room between two crossing ways allows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Step {
    Full,
    Thin,
    Closed,
    AtGrade,
    /// The heights put the lower way over the upper one: drawn as `Closed`.
    Inverted,
}

/// The step of room `r` between the rooms `full` and `least`, with `snap` the at-grade band.
pub fn step(r: f64, full: f64, least: f64, snap: f64) -> Step {
    if r < -snap {
        Step::Inverted
    } else if r <= snap {
        Step::AtGrade
    } else if r < least {
        Step::Closed
    } else if r < full {
        Step::Thin
    } else {
        Step::Full
    }
}

/// The ladder of a crossing: the roles, the room and its step, and on step 3 the way that ends.
#[derive(Clone, Debug, PartialEq)]
pub struct Ladder {
    pub upper: u32,
    pub lower: u32,
    pub room_m: f64,
    pub full_m: f64,
    pub least_m: f64,
    pub step: Step,
    /// On `Closed` and `Inverted`: the way that ends; None over water (a culvert).
    pub ends: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum State {
    Deck,
    Bore,
    /// A ground way.
    Open,
    /// Not drawn: storey 0 over it is whole.
    Hidden,
}

/// A stretch of one state of a way, from station to station.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub way: u32,
    pub from: Station,
    pub to: Station,
    pub state: State,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum EndKind {
    A,
    Portal,
    Closed,
}

/// One straight line across a way where its state changes, normal to its curve there.
#[derive(Clone, Debug, PartialEq)]
pub struct EndLine {
    pub kind: EndKind,
    pub way: u32,
    pub at: Station,
    /// Frame units from the cell's origin.
    pub ends: [[i32; 2]; 2],
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Levels {
    pub runs: Vec<Run>,
    pub end_lines: Vec<EndLine>,
}

/// A body read past the widest half-width: the widest class reach and widening.
const BODY_M: f64 = 12.0;
/// The farthest read about a station: a bore's side or a deck stroke, and a body there.
const GRID_M: f64 = HALF_MAX_M + BODY_M + 13.0;
/// An A line lies at most this far into its deck from the tag end; farther, the way is ground.
const A_MAX_M: f64 = 40.0;
/// The steps of the search for an end line A, metres.
const A_STEP_M: f64 = 0.1;
/// End lines are placed to this, metres.
const PLACE_TO_M: f64 = 0.01;

/// The nearest point of a way's centre polyline: segment `k` (sample index), share `t`, distance and
/// metres across in frame units.
#[derive(Clone, Copy, Debug)]
struct Foot {
    way: u32,
    k: usize,
    t: f64,
    d: f64,
    across: f64,
}

/// A point on a way at a station, in global frame units.
#[derive(Clone, Copy, Debug)]
struct At {
    st: Station,
    p: [f64; 2],
    tangent: [f64; 2],
    z: f64,
    bank: f64,
    widen: f64,
}

fn sub(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn dot(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

fn cross(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

fn add(a: [f64; 2], b: [f64; 2], s: f64) -> [f64; 2] {
    [a[0] + s * b[0], a[1] + s * b[1]]
}

/// The distance of p from segment ab, and the share of its foot.
fn seg_foot(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> (f64, f64) {
    let ab = sub(b, a);
    let l2 = dot(ab, ab);
    let t = if l2 > 0.0 { (dot(sub(p, a), ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
    let e = sub(p, add(a, ab, t));
    (dot(e, e).sqrt(), t)
}

/// The distance between segments ab and cd.
fn segs_dist(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> f64 {
    let side = |p, q, r| cross(sub(q, p), sub(r, p)).signum();
    if side(a, b, c) * side(a, b, d) < 0.0 && side(c, d, a) * side(c, d, b) < 0.0 {
        return 0.0;
    }
    let f = |p, q, r| seg_foot(p, q, r).0;
    f(a, c, d).min(f(b, c, d)).min(f(c, a, b)).min(f(d, a, b))
}

struct Ctx<'a> {
    g: &'a WayGraph,
    input: &'a CellInput,
    rules: Vec<Rule<'a>>,
    s: &'a Structures,
    origin: [f64; 2],
    /// Sample segments near a way with a state, by bucket: (bucket, first sample index).
    grid: Vec<([i64; 2], u32)>,
    /// The way of each sample.
    way_of: Vec<u32>,
    size: f64,
    /// The other arms at each end of a way: (way, at its start); None where the graph holds no point.
    nodes: Vec<Option<Vec<(u32, bool)>>>,
    lengths: Vec<f64>,
}

impl<'a> Ctx<'a> {
    fn new(g: &'a WayGraph, input: &'a CellInput, sections: &'a Sections, s: &'a Structures) -> Ctx<'a> {
        let o = g.cell.origin();
        let origin = [o.x as f64, o.y as f64];
        let rules = (g.parts.iter())
            .map(|p| classify(&input.features[p.feature.0 as usize], sections).expect("a part of a line feature").3)
            .collect();
        let mut nodes: Vec<Option<Vec<(u32, bool)>>> = vec![None; 2 * g.ways.len()];
        let sets = (g.junctions.iter().map(|j| j.arms.iter().collect::<Vec<_>>()))
            .chain(g.joints.iter().map(|j| j.arms.iter().collect()))
            .chain(g.ends.iter().map(|e| vec![&e.arm]));
        for arms in sets {
            for a in &arms {
                let others = arms.iter().filter(|b| !std::ptr::eq(**b, *a)).map(|b| (b.way, b.dir > 0)).collect();
                nodes[2 * a.way as usize + usize::from(a.dir > 0)] = Some(others);
            }
        }
        let mut way_of = vec![0; g.samples.len()];
        for w in &g.ways {
            way_of[w.samples.start as usize..w.samples.end as usize].fill(w.id);
        }
        Ctx {
            g,
            input,
            rules,
            s,
            origin,
            grid: Vec::new(),
            way_of,
            size: super::BUCKET_M / metres_per_unit_at(origin[1]),
            nodes,
            lengths: g.ways.iter().map(|w| between(&g.parts, &w.from, &w.to)).collect(),
        }
    }

    fn others(&self, w: u32, start: bool) -> Option<&[(u32, bool)]> {
        self.nodes[2 * w as usize + usize::from(start)].as_deref()
    }

    fn rule(&self, w: u32) -> &Rule<'a> {
        &self.rules[self.g.ways[w as usize].part as usize]
    }

    fn samples(&self, w: u32) -> &'a [Sample] {
        let r = &self.g.ways[w as usize].samples;
        &self.g.samples[r.start as usize..r.end as usize]
    }

    fn global(&self, s: &Sample) -> [f64; 2] {
        [self.origin[0] + f64::from(s.pos[0]), self.origin[1] + f64::from(s.pos[1])]
    }

    fn mpu(&self, p: [f64; 2]) -> f64 {
        metres_per_unit_at(p[1])
    }

    fn water(&self, w: u32) -> bool {
        self.g.ways[w as usize].family == Family::Water
    }

    /// A bridge way of the tiles (not a synthetic deck) or a tunnel way that is not water.
    fn tagged(&self, w: u32) -> bool {
        self.deck_tagged(w) || self.tunnel(w)
    }

    fn deck_tagged(&self, w: u32) -> bool {
        let way = &self.g.ways[w as usize];
        way.level == Level::Deck && self.g.parts[way.part as usize].level == Level::Deck
    }

    fn tunnel(&self, w: u32) -> bool {
        self.g.ways[w as usize].level == Level::Tunnel && !self.water(w)
    }

    fn station(&self, w: u32, s: &Sample) -> Station {
        Station { part: self.g.ways[w as usize].part, span: s.span, u: s.u }
    }

    fn at_station(&self, st: Station) -> At {
        let part = &self.g.parts[st.part as usize];
        let sp = point(part, st.span as usize, st.u);
        At { st, p: exact(&self.g.parts, &st), tangent: sp.tangent, z: sp.z_m, bank: sp.bank, widen: sp.widen_m }
    }

    /// The point `d` metres along the part of a station, at most to the part's ends.
    fn along(&self, st: &Station, d: f64) -> At {
        let part = &self.g.parts[st.part as usize];
        let n = part.spans.len() as u32;
        let to = Station { part: st.part, span: n - 1, u: 1.0 };
        let whole = Piece {
            part: st.part as usize,
            from: Station { part: st.part, span: 0, u: 0.0 },
            to,
            level: Level::Ground,
            link: None,
            synthetic: false,
        };
        self.at_station(advance(&self.g.parts, &whole, st, d))
    }

    fn at(&self, w: u32, m: f64) -> At {
        self.along(&self.g.ways[w as usize].from, m.min(self.lengths[w as usize]))
    }

    /// The paved half-width of a way, metres.
    fn half(&self, w: u32, widen: f64) -> f64 {
        match self.rule(w) {
            Rule::Way(s, _) => s.half_m + widen,
            Rule::Water(r) => r.half_m,
        }
    }

    fn snap_m(&self, w: u32) -> f64 {
        match self.rule(w) {
            Rule::Way(s, _) => s.snap_height_m,
            Rule::Water(_) => 0.0,
        }
    }

    /// The nearest point of each way within `reach_m` of q, in way order: the one search of the stage.
    fn feet(&self, q: [f64; 2], reach_m: f64) -> Vec<Foot> {
        let r = reach_m / self.mpu(q);
        let b = |v: f64| (v / self.size).floor() as i64;
        let mut ks: Vec<u32> = Vec::new();
        for y in b(q[1] - r)..=b(q[1] + r) {
            for x in b(q[0] - r)..=b(q[0] + r) {
                let from = self.grid.partition_point(|e| e.0 < [x, y]);
                ks.extend(self.grid[from..].iter().take_while(|e| e.0 == [x, y]).map(|e| e.1));
            }
        }
        ks.sort_unstable();
        ks.dedup();
        let mut best: Vec<Foot> = Vec::new();
        for k in ks {
            let (a, c) = (&self.g.samples[k as usize], &self.g.samples[k as usize + 1]);
            let (pa, pc) = (self.global(a), self.global(c));
            let (d, t) = seg_foot(q, pa, pc);
            let ab = sub(pc, pa);
            let l = dot(ab, ab).sqrt();
            let u = if l > 0.0 { [ab[0] / l, ab[1] / l] } else { a.tangent };
            let f = Foot { way: self.way_of[k as usize], k: k as usize, t, d, across: -cross(u, sub(q, pa)) };
            match best.iter_mut().find(|b| b.way == f.way) {
                Some(b) if f.d < b.d => *b = f,
                Some(_) => {}
                None => best.push(f),
            }
        }
        best.sort_by_key(|f| f.way);
        best
    }

    /// The surface of the way at its foot: its curve's height there plus the bank times the metres across.
    fn surface(&self, f: &Foot, q: [f64; 2]) -> f64 {
        let (a, b) = (&self.g.samples[f.k], &self.g.samples[f.k + 1]);
        let lerp = |x: f64, y: f64| x + f.t * (y - x);
        lerp(a.z_m, b.z_m) + lerp(a.bank, b.bank) * f.across * self.mpu(q)
    }

    fn foot_half(&self, f: &Foot) -> f64 {
        let (a, b) = (&self.g.samples[f.k], &self.g.samples[f.k + 1]);
        self.half(f.way, a.widen_m + f.t * (b.widen_m - a.widen_m))
    }

    fn terrain(&self, q: [f64; 2]) -> f64 {
        let p = GlobalPoint { x: libm::round(q[0]) as i64, y: libm::round(q[1]) as i64 };
        self.input.terrain.height(p).unwrap_or(f64::NEG_INFINITY)
    }

    /// Whether way o meets way w at one of w's ends: it goes on from w, it does not lie over it.
    fn joined(&self, w: u32, o: u32) -> bool {
        [true, false].iter().any(|&start| self.others(w, start).is_some_and(|x| x.iter().any(|a| a.0 == o)))
    }

    /// Storey 0 at q without way `not` and the ways that go on from its ends: the terrain and the bodies
    /// of the ground ways near q (their surface less their `fill` times the metres past the paved
    /// stroke). And whether a paved stroke covers q.
    fn ground(&self, q: [f64; 2], not: u32) -> (f64, bool) {
        let mpu = self.mpu(q);
        let (mut z, mut paved) = (self.terrain(q), false);
        for f in self.feet(q, HALF_MAX_M + BODY_M) {
            if f.way == not || self.g.ways[f.way as usize].level != Level::Ground || self.joined(not, f.way) {
                continue;
            }
            let Rule::Way(s, _) = self.rule(f.way) else { continue };
            let past = f.d * mpu - self.foot_half(&f);
            if past <= s.reach_m {
                paved |= past <= 0.0;
                z = z.max(self.surface(&f, q) - s.fill * past.max(0.0));
            }
        }
        (z, paved)
    }

    /// Whether q lies over the body of way `g`: its paved stroke and, for a ground way, its bank down
    /// to the terrain.
    fn in_body(&self, g: u32, q: [f64; 2]) -> bool {
        let Some(f) = self.feet(q, HALF_MAX_M + BODY_M).into_iter().find(|f| f.way == g) else { return false };
        let past = f.d * self.mpu(q) - self.foot_half(&f);
        match self.rule(g) {
            _ if past <= 0.0 => true,
            Rule::Way(s, _) if self.g.ways[g as usize].level == Level::Ground && s.fill > 0.0 => {
                past <= ((self.surface(&f, q) - self.terrain(q)) / s.fill).clamp(0.0, s.reach_m)
            }
            _ => false,
        }
    }

    /// The bore test at a point of a tunnel way: `c` less what it needs (the least room, or that less
    /// `lift_max_m` on free ground). `c` is the least of storey 0 over the bore's width less the floor.
    fn bore(&self, w: u32, a: &At) -> f64 {
        let Rule::Way(s, _) = self.rule(w) else { return f64::NEG_INFINITY };
        let half = (s.half_m + a.widen) / self.mpu(a.p);
        let n = [-a.tangent[1], a.tangent[0]];
        let (mut c, mut free) = (f64::INFINITY, true);
        for side in [-1.0, 0.0, 1.0] {
            let q = add(a.p, n, side * half);
            let (z, paved) = self.ground(q, w);
            let across = -cross(a.tangent, sub(q, a.p)) * self.mpu(a.p);
            c = c.min(z - (a.z + a.bank * across));
            free &= !paved;
        }
        let least = s.bore_least_m + s.roof_least_m;
        c - if free { least - self.s.lift_max_m } else { least }
    }

    /// The state of a tunnel station by its own test: BORE, OPEN or HIDDEN. OPEN needs the floor at
    /// most `snap_height_m` over the terrain: at a tag end the floor lies on the ground way that goes on.
    fn tunnel_state(&self, w: u32, a: &At, tag_m: f64) -> State {
        if self.bore(w, a) >= 0.0 {
            State::Bore
        } else if tag_m > self.s.open_max_m || a.z > self.terrain(a.p) + self.snap_m(w) || self.ground(a.p, w).1 {
            State::Hidden
        } else {
            State::Open
        }
    }

    /// Metres from an end of way w on through the ways that `same` accepts to the first end where
    /// none goes on: 0 there; infinite where the graph does not hold the point or past `limit`.
    fn onward(&self, w: u32, start: bool, limit: f64, same: &dyn Fn(u32) -> bool, depth: u32) -> f64 {
        let Some(others) = self.others(w, start) else { return f64::INFINITY };
        let mut best = f64::INFINITY;
        let mut any = false;
        for &(o, o_start) in others.iter().filter(|(o, _)| same(*o)) {
            any = true;
            let len = self.lengths[o as usize];
            let rest = if len >= limit || depth >= 8 {
                f64::INFINITY
            } else {
                self.onward(o, !o_start, limit - len, same, depth + 1)
            };
            best = best.min(len + rest);
        }
        if any {
            best
        } else {
            0.0
        }
    }

    /// The metres to the tag end behind and ahead of a way of the same kind, read up to `limit`.
    fn tag_ends(&self, w: u32, limit: f64) -> (f64, f64) {
        let same: &dyn Fn(u32) -> bool = if self.tunnel(w) { &|o| self.tunnel(o) } else { &|o| self.deck_tagged(o) };
        (self.onward(w, true, limit, same, 0), self.onward(w, false, limit, same, 0))
    }

    /// How far the deck stroke at metres m stands clear of the paved strokes of the arms, metres.
    fn clear(&self, w: u32, m: f64, arms: &[u32]) -> f64 {
        let a = self.at(w, m);
        let mpu = self.mpu(a.p);
        let h = (self.half(w, a.widen) + self.s.edge_band_m) / mpu;
        let n = [-a.tangent[1], a.tangent[0]];
        let (p1, p2) = (add(a.p, n, h), add(a.p, n, -h));
        let mut best = f64::INFINITY;
        for f in self.feet(a.p, h * mpu + HALF_MAX_M).into_iter().filter(|f| arms.contains(&f.way)) {
            // The way's segments near its foot: its stroke can come nearer the line than the foot.
            let r = &self.g.ways[f.way as usize].samples;
            for k in f.k.saturating_sub(8).max(r.start as usize)..(f.k + 9).min(r.end as usize - 1) {
                let (sa, sb) = (&self.g.samples[k], &self.g.samples[k + 1]);
                let d = segs_dist(p1, p2, self.global(sa), self.global(sb)) * mpu;
                best = best.min(d - self.half(f.way, sa.widen_m.max(sb.widen_m)));
            }
        }
        best
    }
}

/// The levels of a graph: the ladders of its crossings, then the states and end lines of its ways.
pub fn levels(g: &mut WayGraph, input: &CellInput, sections: &Sections, s: &Structures) {
    let (ladders, levels) = {
        let mut ctx = Ctx::new(g, input, sections, s);
        let ladders: Vec<Option<Ladder>> = g.crossings.iter().map(|c| ladder(&ctx, c)).collect();
        ctx.grid = grid(&ctx, &ladders);
        let levels = states(&ctx, &ladders);
        (ladders, levels)
    };
    for (c, l) in g.crossings.iter_mut().zip(ladders) {
        c.ladder = l;
    }
    g.levels = levels;
}

/// The sample segments of every way near a way that gets a state, in buckets: the reads of the
/// states reach no farther than `GRID_M` from such a way.
fn grid(ctx: &Ctx, ladders: &[Option<Ladder>]) -> Vec<([i64; 2], u32)> {
    let g = ctx.g;
    let ended = ladders.iter().flatten().filter_map(|l| l.ends);
    let mut boxes: Vec<[f64; 4]> = Vec::new();
    for w in (0..g.ways.len() as u32).filter(|&w| ctx.tagged(w)).chain(ended) {
        let pts: Vec<[f64; 2]> = ctx.samples(w).iter().map(|s| ctx.global(s)).collect();
        let grow = GRID_M / ctx.mpu(pts[0]);
        let lo = |i: usize| pts.iter().map(|p| p[i]).fold(f64::INFINITY, f64::min) - grow;
        let hi = |i: usize| pts.iter().map(|p| p[i]).fold(f64::NEG_INFINITY, f64::max) + grow;
        boxes.push([lo(0), lo(1), hi(0), hi(1)]);
    }
    let mut grid = Vec::new();
    for w in g.ways.iter().filter(|_| !boxes.is_empty()) {
        for k in w.samples.start..w.samples.end.saturating_sub(1) {
            let (a, b) = (ctx.global(&g.samples[k as usize]), ctx.global(&g.samples[k as usize + 1]));
            let near = |x: &[f64; 4]| {
                a[0].max(b[0]) >= x[0] && a[0].min(b[0]) <= x[2] && a[1].max(b[1]) >= x[1] && a[1].min(b[1]) <= x[3]
            };
            if !boxes.iter().any(near) {
                continue;
            }
            let lo = |i: usize| (a[i].min(b[i]) / ctx.size).floor() as i64;
            let hi = |i: usize| (a[i].max(b[i]) / ctx.size).floor() as i64;
            for y in lo(1)..=hi(1) {
                for x in lo(0)..=hi(0) {
                    grid.push(([x, y], k));
                }
            }
        }
    }
    grid.sort_unstable();
    grid
}

/// The way of a crossing's station.
fn way_of(g: &WayGraph, st: &Station) -> Option<u32> {
    let inside = |w: &&Way| w.part == st.part && key(&w.from) <= key(st) && key(st) <= key(&w.to);
    g.ways.iter().find(inside).map(|w| w.id)
}

/// The deck depth of a bridge way's tile tag, else the thinnest deck of its class.
fn deck_depth(ctx: &Ctx, w: u32) -> f64 {
    let Rule::Way(s, _) = ctx.rule(w) else { return 0.0 };
    let part = &ctx.g.parts[ctx.g.ways[w as usize].part as usize];
    let tag = ctx.input.features[part.feature.0 as usize].tags.get("deck");
    tag.and_then(|t| leading_number(t)).filter(|_| ctx.deck_tagged(w)).unwrap_or(s.deck_least_m)
}

fn ladder(ctx: &Ctx, c: &super::Crossing) -> Option<Ladder> {
    let g = ctx.g;
    let (upper, lower) = (way_of(g, &c.over)?, way_of(g, &c.under)?);
    let water = ctx.water(lower);
    let rated = match c.kind {
        CrossingKind::Bore | CrossingKind::Stacked => !ctx.water(upper) && !water,
        CrossingKind::Deck | CrossingKind::Footbridge => !ctx.water(upper),
        CrossingKind::LevelCrossing => true,
        _ => false,
    };
    if !rated {
        return None;
    }
    // The corners of the overlap of the two paved strokes, each on its way's own curve: along the upper
    // way by the lower one's half-width over the sine, across it by its own; and the same for the lower.
    let (au, al) = (ctx.at_station(c.over), ctx.at_station(c.under));
    let cs = cross(au.tangent, al.tangent);
    let sin = cs.abs().max(1e-9);
    let (hu, hl) = (ctx.half(upper, au.widen), ctx.half(lower, al.widen));
    let (lu, ll) = ((hl / sin).min(PLACE_M), (hu / sin).min(PLACE_M));
    let mut r = f64::INFINITY;
    for (a, b) in [(0.0, 0.0), (1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)] {
        let (pu, pl) = (ctx.along(&c.over, a * lu), ctx.along(&c.under, b * ll));
        let zu = pu.z + pu.bank * (-b * ll * cs);
        let zl = pl.z + pl.bank * (a * lu * cs);
        r = r.min(zu - zl);
    }
    let Rule::Way(su, _) = ctx.rule(upper) else { return None };
    let (full, least) = match ctx.rule(lower) {
        Rule::Water(w) => (w.clearance_m + deck_depth(ctx, upper), w.clearance_least_m + su.deck_least_m),
        Rule::Way(sl, _) if ctx.tunnel(lower) => (sl.bore_m + ctx.s.cover_m, sl.bore_least_m + sl.roof_least_m),
        Rule::Way(sl, _) => (sl.clearance_m + deck_depth(ctx, upper), sl.clearance_least_m + su.deck_least_m),
    };
    let snap = if water { su.snap_height_m } else { su.snap_height_m.min(ctx.snap_m(lower)) };
    let step = if c.kind == CrossingKind::LevelCrossing { Step::AtGrade } else { step(r, full, least, snap) };
    let ends = match step {
        Step::Closed | Step::Inverted if !water => Some(ends(ctx, c, upper, lower)),
        _ => None,
    };
    Some(Ladder { upper, lower, room_m: r, full_m: full, least_m: least, step, ends })
}

/// The way that ends on step 3: a tunnel way; else the later row of `roads.json`; a tie, the lower
/// way; with no tag and one height, the way whose tile span comes later.
fn ends(ctx: &Ctx, c: &super::Crossing, upper: u32, lower: u32) -> u32 {
    let (tu, tl) = (ctx.tunnel(upper), ctx.tunnel(lower));
    if tu != tl {
        return if tu { upper } else { lower };
    }
    let (cu, cl) = (ctx.g.ways[upper as usize].class, ctx.g.ways[lower as usize].class);
    if cu != cl {
        return if cu > cl { upper } else { lower };
    }
    if ctx.tagged(upper) || ctx.tagged(lower) || ctx.at_station(c.over).z != ctx.at_station(c.under).z {
        return lower;
    }
    let owner = |st: &Station| ctx.g.parts[st.part as usize].spans[st.span as usize].owner;
    if owner(&c.over) > owner(&c.under) {
        upper
    } else {
        lower
    }
}

/// A DECK way's A lines (metres along) at its tag ends and the metres between them, read on through
/// deck joints up to `deck_min_m`; None where an A lies past `A_MAX_M`.
type DeckEnds = ([Option<f64>; 2], f64);

fn deck_ends(ctx: &Ctx, w: u32, ground: &[bool]) -> Option<DeckEnds> {
    let len = ctx.lengths[w as usize];
    let deck = |o: u32| ctx.deck_tagged(o) && !ground[o as usize];
    let mut a = [None, None];
    for (i, start) in [(0, true), (1, false)] {
        let Some(others) = ctx.others(w, start) else { continue };
        if others.iter().any(|(o, _)| deck(*o)) {
            continue;
        }
        let arms: Vec<u32> = others.iter().map(|(o, _)| *o).filter(|&o| !ctx.water(o)).collect();
        a[i] = Some(place_a(ctx, w, start, &arms, len)?);
    }
    let mut between_m = a[1].unwrap_or(len) - a[0].unwrap_or(0.0);
    for (i, start) in [(0, true), (1, false)] {
        if a[i].is_none() {
            between_m += ctx.onward(w, start, ctx.s.deck_min_m, &deck, 0).min(ctx.s.deck_min_m);
        }
    }
    Some((a, between_m))
}

/// The first `m` in [lo, hi] where `ok` turns true, to `PLACE_TO_M`; `ok(lo)` is false, `ok(hi)` true.
fn bisect(mut lo: f64, mut hi: f64, ok: &dyn Fn(f64) -> bool) -> f64 {
    while hi - lo > PLACE_TO_M {
        let mid = (lo + hi) / 2.0;
        if ok(mid) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    hi
}

/// An A line: from the tag end inward to the first sample where the deck stroke is clear of every
/// ground arm, then back in steps of `A_STEP_M` and bisected; None past `A_MAX_M`.
fn place_a(ctx: &Ctx, w: u32, start: bool, arms: &[u32], len: f64) -> Option<f64> {
    let at = |d: f64| if start { d } else { len - d };
    let ok = |d: f64| ctx.clear(w, at(d), arms) > 0.0;
    if ok(0.0) {
        return Some(at(0.0));
    }
    let from = ctx.g.ways[w as usize].from;
    let mut ds: Vec<f64> = ctx.samples(w).iter().map(|s| between(&ctx.g.parts, &from, &ctx.station(w, s))).collect();
    if !start {
        ds = ds.iter().rev().map(|m| len - m).collect();
    }
    let hit = ds.iter().copied().filter(|&d| d > 0.0 && d <= A_MAX_M).find(|&d| ok(d))?;
    let mut d = ds.iter().copied().filter(|&x| x < hit).fold(0.0, f64::max);
    loop {
        let next = (d + A_STEP_M).min(hit);
        if ok(next) {
            return Some(at(bisect(d, next, &ok)));
        }
        d = next;
    }
}

/// Per way with a state: its samples' metres along, states and tag-end metres, and its A lines.
struct Track {
    way: u32,
    m: Vec<f64>,
    state: Vec<State>,
    tag: (f64, f64),
    a: [Option<f64>; 2],
}

impl Track {
    fn tag_m(&self, len: f64, m: f64) -> f64 {
        (self.tag.0 + m).min(self.tag.1 + len - m)
    }
}

fn states(ctx: &Ctx, ladders: &[Option<Ladder>]) -> Levels {
    let g = ctx.g;
    // The step-3 crossings by the way that ends: (crossing, the way that stays).
    let mut ended: Vec<Vec<(usize, u32)>> = vec![Vec::new(); g.ways.len()];
    let mut culvert = vec![false; g.ways.len()];
    for (i, l) in ladders.iter().enumerate() {
        let Some(l) = l.as_ref().filter(|l| matches!(l.step, Step::Closed | Step::Inverted)) else { continue };
        match l.ends {
            Some(e) => ended[e as usize].push((i, if e == l.upper { l.lower } else { l.upper })),
            None => culvert[l.upper as usize] = true,
        }
    }
    let ways: Vec<u32> = (0..g.ways.len() as u32).filter(|&w| ctx.tagged(w) || !ended[w as usize].is_empty()).collect();
    // Deck ways that are ground: over a culvert, or with no room for a deck between their A lines. A
    // deck way that turns ground changes the A lines of the deck ways at its ends only.
    let mut ground: Vec<bool> = (0..g.ways.len() as u32).map(|w| !ctx.deck_tagged(w) || culvert[w as usize]).collect();
    let mut deck_of: Vec<Option<DeckEnds>> = vec![None; g.ways.len()];
    let mut dirty: Vec<u32> = ways.iter().copied().filter(|&w| !ground[w as usize]).collect();
    for _ in 0..4 {
        let mut turned = Vec::new();
        for &w in &dirty {
            let ends = deck_ends(ctx, w, &ground);
            if ends.is_none_or(|e| e.1 < ctx.s.deck_min_m) {
                ground[w as usize] = true;
                turned.push(w);
            }
            deck_of[w as usize] = ends;
        }
        let next = turned.iter().flat_map(|&w| [(w, true), (w, false)]).flat_map(|(w, s)| ctx.others(w, s)).flatten();
        dirty = next.map(|&(o, _)| o).filter(|&o| !ground[o as usize]).collect();
        dirty.sort_unstable();
        dirty.dedup();
        if dirty.is_empty() {
            break;
        }
    }
    let mut out = Levels::default();
    for &w in &ways {
        let samples = ctx.samples(w);
        let from = g.ways[w as usize].from;
        let m: Vec<f64> = samples.iter().map(|s| between(&g.parts, &from, &ctx.station(w, s))).collect();
        let mut t = Track { way: w, state: vec![State::Open; m.len()], tag: (0.0, 0.0), a: [None, None], m };
        if ctx.tunnel(w) {
            t.tag = ctx.tag_ends(w, ctx.s.open_max_m);
            for (i, s) in samples.iter().enumerate() {
                t.state[i] =
                    ctx.tunnel_state(w, &ctx.at_station(ctx.station(w, s)), t.tag_m(ctx.lengths[w as usize], t.m[i]));
            }
            let raw = t.state.clone();
            for i in 0..t.state.len() {
                t.state[i] = open_rule(&t, &raw, ctx.lengths[w as usize], i);
            }
        } else if !ground[w as usize] {
            let (a, _) = deck_of[w as usize].expect("each DECK way has its ends");
            let (lo, hi) = (a[0].unwrap_or(0.0), a[1].unwrap_or(ctx.lengths[w as usize]));
            for (s, &m) in t.state.iter_mut().zip(&t.m) {
                if m >= lo && m <= hi {
                    *s = State::Deck;
                }
            }
            t.a = a;
        }
        // Step 3: not drawn over the body of the way that stays; the rest of the way keeps its state.
        for &(i, stay) in &ended[w as usize] {
            let c = &g.crossings[i];
            let st = if way_of(g, &c.over) == Some(w) { c.over } else { c.under };
            let k0 = t.m.partition_point(|&x| x < between(&g.parts, &from, &st));
            let over = |k: usize| ctx.in_body(stay, ctx.global(&samples[k]));
            for k in (k0..samples.len()).take_while(|&k| over(k)) {
                t.state[k] = State::Hidden;
            }
            for k in (0..k0).rev().take_while(|&k| over(k)) {
                t.state[k] = State::Hidden;
            }
        }
        runs(ctx, &t, &ended[w as usize], &mut out);
    }
    out
}

/// OPEN only between a tag end and the first BORE from that end: the read stays within `open_max_m`.
fn open_rule(t: &Track, raw: &[State], len: f64, i: usize) -> State {
    if raw[i] != State::Open {
        return raw[i];
    }
    let back = t.tag.0 + t.m[i] <= t.tag.1 + len - t.m[i];
    let between = if back { &raw[..i] } else { &raw[i + 1..] };
    if between.contains(&State::Bore) {
        State::Hidden
    } else {
        State::Open
    }
}

/// The runs and end lines of a track: a change between two samples lies where the state that tells
/// the two sides apart begins, bisected; an A lies where `deck_ends` put it.
fn runs(ctx: &Ctx, t: &Track, ended: &[(usize, u32)], out: &mut Levels) {
    let g = ctx.g;
    let w = t.way;
    let len = ctx.lengths[w as usize];
    let tunnel = ctx.tunnel(w);
    let stays: Vec<u32> = ended.iter().map(|&(_, s)| s).collect();
    let state_at = |m: f64| -> State {
        let a = ctx.at(w, m);
        if stays.iter().any(|&s| ctx.in_body(s, a.p)) {
            State::Hidden
        } else if tunnel {
            // The OPEN rule at m: no BORE sample between m and its nearer tag end.
            let s = ctx.tunnel_state(w, &a, t.tag_m(len, m));
            let back = t.tag.0 + m <= t.tag.1 + len - m;
            let bore = t.m.iter().zip(&t.state).any(|(&x, &st)| st == State::Bore && if back { x < m } else { x > m });
            if s == State::Open && bore {
                State::Hidden
            } else {
                s
            }
        } else {
            State::Open
        }
    };
    let mut from = g.ways[w as usize].from;
    let mut first = 0;
    for i in 1..=t.state.len() {
        if i < t.state.len() && t.state[i] == t.state[i - 1] {
            continue;
        }
        let state = t.state[first];
        let to = if i == t.state.len() {
            g.ways[w as usize].to
        } else {
            let (m0, m1, next) = (t.m[i - 1], t.m[i], t.state[i]);
            let a_at = t.a.iter().flatten().copied().find(|&x| x > m0 && x <= m1);
            let deck_change = matches!((state, next), (State::Open, State::Deck) | (State::Deck, State::Open));
            let m = match a_at {
                Some(x) if deck_change => x,
                _ => {
                    let tell = if state == State::Bore || next == State::Bore {
                        State::Bore
                    } else if state == State::Deck || next == State::Deck {
                        State::Deck
                    } else {
                        State::Hidden
                    };
                    let member = |s: State| if tell == State::Deck { s != State::Hidden } else { s == tell };
                    let side = member(state);
                    bisect(m0, m1, &|m| member(state_at(m)) != side)
                }
            };
            let kind = match (state, next) {
                (State::Hidden, _) | (_, State::Hidden) => EndKind::Closed,
                (State::Deck, _) | (_, State::Deck) => EndKind::A,
                _ => EndKind::Portal,
            };
            let st = ctx.at(w, m).st;
            out.end_lines.push(line_at(ctx, w, st, kind, state.min(next)));
            st
        };
        out.runs.push(Run { way: w, from, to, state });
        from = to;
        first = i;
    }
    // A change at a tag end: the way's end state against the ground way beyond it.
    for (start, k) in [(true, 0), (false, t.state.len() - 1)] {
        let state = t.state[k];
        let Some(others) = ctx.others(w, start) else { continue };
        let same = |o: u32| if tunnel { ctx.tunnel(o) } else { ctx.deck_tagged(o) };
        if state == State::Open || others.iter().any(|(o, _)| same(*o)) {
            continue;
        }
        let end_m = if start { 0.0 } else { len };
        if state == State::Deck && !t.a[usize::from(!start)].is_some_and(|x| (x - end_m).abs() < 1e-9) {
            continue;
        }
        let kind = match state {
            State::Deck => EndKind::A,
            State::Bore => EndKind::Portal,
            _ => EndKind::Closed,
        };
        let st = if start { g.ways[w as usize].from } else { g.ways[w as usize].to };
        out.end_lines.push(line_at(ctx, w, st, kind, state));
    }
}

/// The end line of one way at a station: across the stroke of the structure side.
fn line_at(ctx: &Ctx, w: u32, st: Station, kind: EndKind, structure: State) -> EndLine {
    let a = ctx.at_station(st);
    let band = if matches!(structure, State::Deck | State::Bore) { ctx.s.edge_band_m } else { 0.0 };
    let h = (ctx.half(w, a.widen) + band) / ctx.mpu(a.p);
    let n = [-a.tangent[1], a.tangent[0]];
    let local = |p: [f64; 2]| [(libm::round(p[0]) - ctx.origin[0]) as i32, (libm::round(p[1]) - ctx.origin[1]) as i32];
    EndLine { kind, way: w, at: st, ends: [local(add(a.p, n, h)), local(add(a.p, n, -h))] }
}

/// G7: the runs of a bridge or tunnel way cover it once, in order; OPEN lies within `open_max_m` of a
/// tag end and never between two BORE runs. G8: each BORE sample has its room; a DECK way has
/// `deck_min_m` between its end lines. G9: an A line lies outside the paved stroke of every ground arm.
pub fn check_levels(g: &WayGraph, input: &CellInput, sections: &Sections, s: &Structures) -> Vec<Fault> {
    let ctx = {
        let mut c = Ctx::new(g, input, sections, s);
        let ladders: Vec<Option<Ladder>> = g.crossings.iter().map(|x| x.ladder.clone()).collect();
        c.grid = grid(&c, &ladders);
        c
    };
    let mut faults = Vec::new();
    let mut fault = |check: &'static str, what: String| faults.push(Fault { check, what });
    let tolerance = 1e-6;
    let k = |st: &Station| (st.span, st.u);
    for way in &g.ways {
        let w = way.id;
        let runs: Vec<&Run> = g.levels.runs.iter().filter(|r| r.way == w).collect();
        if !ctx.tagged(w) && runs.is_empty() {
            continue;
        }
        let joined = runs.windows(2).all(|p| p[0].to == p[1].from);
        let ordered = runs.iter().all(|r| k(&r.from) <= k(&r.to));
        if runs.first().map(|r| r.from) != Some(way.from)
            || runs.last().map(|r| r.to) != Some(way.to)
            || !joined
            || !ordered
        {
            fault("G7", format!("way {w}: its runs do not cover it once in order"));
        }
        let len = ctx.lengths[w as usize];
        let tunnel = ctx.tunnel(w);
        let (back, ahead) = ctx.tag_ends(w, s.open_max_m.max(s.deck_min_m));
        let inside = |r: &Run, st: &Station| k(&r.from) <= k(st) && k(st) <= k(&r.to);
        for r in &runs {
            let wrong = match r.state {
                State::Deck => tunnel,
                State::Bore => !tunnel,
                State::Open => {
                    let tag = |st: &Station| {
                        let m = between(&g.parts, &way.from, st);
                        (back + m).min(ahead + len - m)
                    };
                    tunnel && tag(&r.from).min(tag(&r.to)) > s.open_max_m + PLACE_TO_M
                }
                State::Hidden => false,
            };
            if wrong {
                fault("G7", format!("way {w}: a {:?} run at {:?}", r.state, r.from));
            }
        }
        let bores: Vec<usize> = (0..runs.len()).filter(|&i| runs[i].state == State::Bore).collect();
        if let (Some(&a), Some(&b)) = (bores.first(), bores.last()) {
            if runs[a..b].iter().any(|r| r.state == State::Open) {
                fault("G7", format!("way {w}: an OPEN run between two BORE runs"));
            }
        }
        for smp in ctx.samples(w) {
            let st = ctx.station(w, smp);
            if runs.iter().any(|r| r.state == State::Bore && inside(r, &st))
                && ctx.bore(w, &ctx.at_station(st)) < -tolerance
            {
                fault("G8", format!("way {w}: a BORE station at {st:?} lacks room"));
            }
        }
        let deck: f64 = runs.iter().filter(|r| r.state == State::Deck).map(|r| between(&g.parts, &r.from, &r.to)).sum();
        if deck > 0.0 {
            let on = |start: bool| {
                let reaches = runs
                    .iter()
                    .any(|r| r.state == State::Deck && if start { r.from == way.from } else { r.to == way.to });
                if reaches {
                    ctx.onward(w, start, s.deck_min_m, &|o| ctx.deck_tagged(o), 0).min(s.deck_min_m)
                } else {
                    0.0
                }
            };
            if deck + on(true) + on(false) < s.deck_min_m - tolerance {
                fault("G8", format!("way {w}: a deck of {deck} m between its end lines"));
            }
        }
    }
    for line in g.levels.end_lines.iter().filter(|l| l.kind == EndKind::A) {
        let w = line.way;
        let m = between(&g.parts, &g.ways[w as usize].from, &line.at);
        let start = m <= ctx.lengths[w as usize] / 2.0;
        let arms: Vec<u32> = ctx
            .others(w, start)
            .into_iter()
            .flatten()
            .map(|(o, _)| *o)
            .filter(|&o| !ctx.water(o) && !ctx.deck_tagged(o))
            .collect();
        if ctx.clear(w, m, &arms) < -PLACE_TO_M {
            fault("G9", format!("way {w}: an A line at {:?} lies inside a ground arm", line.at));
        }
    }
    faults
}
