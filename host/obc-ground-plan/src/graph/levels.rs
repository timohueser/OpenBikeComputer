//! Stage 6: the levels of bridges and tunnels. Per crossing the room between its two ways and its
//! step of the ladder; per station of a tagged way its state; the end lines where a state changes;
//! the sheets of DECK and BORE pieces with their storeys. It draws nothing: the faces still read
//! `Way::level` and `Way::deck`.
//!
//! Every decision reads the ways within its reach: the bodies within `HALF_MAX_M` and a class reach
//! of a station, the ways along a tagged way up to `open_max_m` or `deck_min_m`, and the samples of
//! the two ways at a crossing within `PLACE_M`.

use std::collections::BTreeMap;

use super::curve::point;
use super::parts::Rule;
use super::points::{advance, between, exact, key, Piece};
use super::{CrossingKind, Family, Level, Part, Sample, Station, Way, WayGraph, PLACE_M};
use crate::frame::{metres_per_unit_at, GlobalPoint};
use crate::sections::{leading_number, HALF_MAX_M};
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
    /// The band past a deck's paved edge, and a bore's wall.
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

    /// The row of the reach table: a station's state reads along its way to a tag end, then the
    /// bodies about the station and the room of a crossing there.
    pub fn reach(&self) -> crate::sections::Reach {
        let terms = [
            ("open_max_m", self.open_max_m.max(self.deck_min_m)),
            ("HALF_MAX_M and a body's reach", HALF_MAX_M + BODY_M),
            ("PLACE_M: the room of a crossing", PLACE_M),
        ];
        crate::sections::Reach {
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
    pub sheet: Option<u32>,
    /// BORE: the least of `c` less what the station needs; DECK: the metres between the end lines,
    /// read on through deck joints up to `deck_min_m`. Else 0.
    pub margin_m: f64,
    /// The least metres along from a station of the run to a tag end, read up to `open_max_m`.
    pub tag_m: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum EndKind {
    A,
    Portal,
    Closed,
}

/// One straight line across the ways that change state there, normal to their mean direction.
#[derive(Clone, Debug, PartialEq)]
pub struct EndLine {
    pub kind: EndKind,
    /// Each way and the station where it crosses the line.
    pub ways: Vec<(u32, Station)>,
    /// Frame units from the cell's origin.
    pub ends: [[i32; 2]; 2],
    /// A: the least distance of the line from the paved stroke of a ground arm. Else 0.
    pub clear_m: f64,
}

/// The DECK or BORE pieces that one overlay builds, and its storey: decks +1, +2 …, bores −1, −2 ….
#[derive(Clone, Debug, PartialEq)]
pub struct Sheet {
    pub runs: Vec<u32>,
    pub storey: i8,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Levels {
    pub runs: Vec<Run>,
    pub end_lines: Vec<EndLine>,
    pub sheets: Vec<Sheet>,
    pub open_max_m: f64,
    pub deck_min_m: f64,
}

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

/// The distance of p from segment ab.
fn seg_dist(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let ab = sub(b, a);
    let l2 = dot(ab, ab);
    let t = if l2 > 0.0 { (dot(sub(p, a), ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
    let e = sub(p, add(a, ab, t));
    dot(e, e).sqrt()
}

/// The distance between segments ab and cd.
fn segs_dist(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> f64 {
    let side = |p, q, r| cross(sub(q, p), sub(r, p)).signum();
    if side(a, b, c) * side(a, b, d) < 0.0 && side(c, d, a) * side(c, d, b) < 0.0 {
        return 0.0;
    }
    seg_dist(a, c, d).min(seg_dist(b, c, d)).min(seg_dist(c, a, b)).min(seg_dist(d, a, b))
}

struct Ctx<'a> {
    g: &'a WayGraph,
    input: &'a CellInput,
    rules: &'a [Rule<'a>],
    s: &'a Structures,
    origin: [f64; 2],
    /// Sample segments by bucket: (bucket, way, first sample index).
    grid: Vec<([i64; 2], u32, u32)>,
    size: f64,
    /// The other arms at each end of a way: (way, at its start).
    nodes: BTreeMap<(u32, bool), Vec<(u32, bool)>>,
    lengths: Vec<f64>,
}

impl<'a> Ctx<'a> {
    fn rule(&self, w: u32) -> &Rule<'a> {
        &self.rules[self.g.ways[w as usize].part as usize]
    }

    fn part(&self, w: u32) -> &Part {
        &self.g.parts[self.g.ways[w as usize].part as usize]
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
        let way = &self.g.ways[w as usize];
        match way.level {
            Level::Deck => self.part(w).level == Level::Deck,
            Level::Tunnel => !self.water(w),
            Level::Ground => false,
        }
    }

    fn deck_tagged(&self, w: u32) -> bool {
        self.g.ways[w as usize].level == Level::Deck && self.tagged(w)
    }

    fn tunnel(&self, w: u32) -> bool {
        self.g.ways[w as usize].level == Level::Tunnel && !self.water(w)
    }

    fn piece(&self, w: u32) -> Piece {
        let way = &self.g.ways[w as usize];
        Piece {
            part: way.part as usize,
            from: way.from,
            to: way.to,
            level: way.level,
            link: way.link,
            synthetic: false,
        }
    }

    fn station(&self, w: u32, s: &Sample) -> Station {
        Station { part: self.g.ways[w as usize].part, span: s.span, u: s.u }
    }

    fn metres(&self, w: u32, st: &Station) -> f64 {
        between(&self.g.parts, &self.g.ways[w as usize].from, st)
    }

    fn at_station(&self, st: Station) -> At {
        let part = &self.g.parts[st.part as usize];
        let sp = point(part, st.span as usize, st.u);
        At { st, p: exact(&self.g.parts, &st), tangent: sp.tangent, z: sp.z_m, bank: sp.bank, widen: sp.widen_m }
    }

    fn at(&self, w: u32, m: f64) -> At {
        let way = &self.g.ways[w as usize];
        self.at_station(advance(&self.g.parts, &self.piece(w), &way.from, m))
    }

    /// The paved half-width of a way, metres.
    fn half(&self, w: u32, widen: f64) -> f64 {
        match self.rule(w) {
            Rule::Way(s, _) => s.half_m + widen,
            Rule::Water(r) => r.half_m,
        }
    }

    fn ways_near(&self, q: [f64; 2], reach_m: f64) -> Vec<(u32, u32)> {
        let r = reach_m / self.mpu(q);
        let b = |v: f64| (v / self.size).floor() as i64;
        let mut out = Vec::new();
        for y in b(q[1] - r)..=b(q[1] + r) {
            for x in b(q[0] - r)..=b(q[0] + r) {
                let from = self.grid.partition_point(|e| e.0 < [x, y]);
                out.extend(self.grid[from..].iter().take_while(|e| e.0 == [x, y]).map(|e| (e.1, e.2)));
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    fn foot_on(&self, w: u32, k: usize, q: [f64; 2]) -> Foot {
        let (a, b) = (&self.g.samples[k], &self.g.samples[k + 1]);
        let (pa, pb) = (self.global(a), self.global(b));
        let ab = sub(pb, pa);
        let l2 = dot(ab, ab);
        let t = if l2 > 0.0 { (dot(sub(q, pa), ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
        let e = sub(q, add(pa, ab, t));
        let u = if l2 > 0.0 { [ab[0] / l2.sqrt(), ab[1] / l2.sqrt()] } else { a.tangent };
        Foot { way: w, k, t, d: dot(e, e).sqrt(), across: -cross(u, sub(q, pa)) }
    }

    /// The nearest point of each way near q: (way, foot), in way order.
    fn feet(&self, q: [f64; 2], reach_m: f64) -> Vec<Foot> {
        let mut best: Vec<Foot> = Vec::new();
        for (w, k) in self.ways_near(q, reach_m) {
            let f = self.foot_on(w, k as usize, q);
            match best.last_mut() {
                Some(b) if b.way == w => {
                    if f.d < b.d {
                        *b = f;
                    }
                }
                _ => best.push(f),
            }
        }
        best
    }

    /// The nearest point of one way to q, over all its samples.
    fn foot(&self, w: u32, q: [f64; 2]) -> Option<Foot> {
        let r = &self.g.ways[w as usize].samples;
        (r.start as usize..r.end as usize - 1)
            .map(|k| self.foot_on(w, k, q))
            .min_by(|a, b| a.d.total_cmp(&b.d).then(a.k.cmp(&b.k)))
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

    /// Storey 0 at q without way `not`: the terrain and the bodies of the ground ways near q (their
    /// surface less their `fill` times the metres past the paved stroke). And whether a paved stroke
    /// covers q.
    fn ground(&self, q: [f64; 2], not: u32) -> (f64, bool) {
        let mpu = self.mpu(q);
        let (mut z, mut paved) = (self.terrain(q), false);
        for f in self.feet(q, HALF_MAX_M + BODY_M) {
            let way = &self.g.ways[f.way as usize];
            if f.way == not || way.level != Level::Ground || self.water(f.way) {
                continue;
            }
            let Rule::Way(s, _) = self.rule(f.way) else { continue };
            let past = f.d * mpu - self.foot_half(&f);
            if past > s.reach_m {
                continue;
            }
            paved |= past <= 0.0;
            z = z.max(self.surface(&f, q) - s.fill * past.max(0.0));
        }
        (z, paved)
    }

    /// Whether q lies over the body of way `g`: its paved stroke and, for a ground way, its bank down
    /// to the terrain.
    fn in_body(&self, g: u32, q: [f64; 2]) -> bool {
        let Some(f) = self.foot(g, q) else { return false };
        let past = f.d * self.mpu(q) - self.foot_half(&f);
        if past <= 0.0 {
            return true;
        }
        match self.rule(g) {
            Rule::Way(s, _) if self.g.ways[g as usize].level == Level::Ground && s.fill > 0.0 => {
                past <= ((self.surface(&f, q) - self.terrain(q)) / s.fill).clamp(0.0, s.reach_m)
            }
            _ => false,
        }
    }

    /// The bore test at a point of a tunnel way: `c` less what it needs (the least room, or that less
    /// `lift_max_m` on free ground), and `c` itself.
    fn bore(&self, w: u32, a: &At) -> (f64, f64) {
        let Rule::Way(s, _) = self.rule(w) else { return (f64::NEG_INFINITY, f64::NEG_INFINITY) };
        let half = (s.half_m + a.widen) / self.mpu(a.p);
        let n = [-a.tangent[1], a.tangent[0]];
        let mut c = f64::INFINITY;
        let mut free = true;
        for side in [-1.0, 0.0, 1.0] {
            let q = add(a.p, n, side * half);
            let (z, paved) = self.ground(q, w);
            // The floor at the side: the bank about the metres across, as `surface` reads it.
            let across = -cross(a.tangent, sub(q, a.p)) * self.mpu(a.p);
            c = c.min(z - (a.z + a.bank * across));
            free &= !paved;
        }
        let least = s.bore_least_m + s.roof_least_m;
        let need = if free { least - self.s.lift_max_m } else { least };
        (c - need, c)
    }

    /// The raw state of a tunnel station: BORE, OPEN or HIDDEN, before the run rules.
    fn tunnel_state(&self, w: u32, a: &At, tag_m: f64) -> State {
        if self.bore(w, a).0 >= 0.0 {
            return State::Bore;
        }
        let (_, paved) = self.ground(a.p, w);
        if a.z <= self.terrain(a.p) && !paved && tag_m <= self.s.open_max_m {
            State::Open
        } else {
            State::Hidden
        }
    }

    /// Metres from an end of way w on through the ways that `same` accepts to the first end where
    /// none goes on: 0 there; infinite where the graph does not hold the point or past `limit`.
    fn onward(&self, w: u32, start: bool, limit: f64, same: &dyn Fn(u32) -> bool, depth: u32) -> f64 {
        let Some(others) = self.nodes.get(&(w, start)) else { return f64::INFINITY };
        let on: Vec<&(u32, bool)> = others.iter().filter(|(o, _)| same(*o)).collect();
        if on.is_empty() {
            return 0.0;
        }
        let mut best = f64::INFINITY;
        for &&(o, o_start) in &on {
            let len = self.lengths[o as usize];
            let rest = if len >= limit || depth >= 8 {
                f64::INFINITY
            } else {
                self.onward(o, !o_start, limit - len, same, depth + 1)
            };
            best = best.min(len + rest);
        }
        best
    }
}

/// A body read past the widest half-width: the widest class reach and widening.
const BODY_M: f64 = 12.0;
/// The steps of the search for an end line A, metres.
const A_STEP_M: f64 = 0.1;
/// End lines are placed to this, metres.
const PLACE_TO_M: f64 = 0.01;

/// The levels of a graph: ladders on its crossings, then states, end lines and sheets.
pub(crate) fn levels(g: &mut WayGraph, input: &CellInput, rules: &[Rule<'_>], s: &Structures) {
    let o = g.cell.origin();
    let origin = [o.x as f64, o.y as f64];
    let size = super::BUCKET_M / metres_per_unit_at(origin[1]);
    let mut grid = Vec::new();
    for w in &g.ways {
        for k in w.samples.start..w.samples.end.saturating_sub(1) {
            let (a, b) = (&g.samples[k as usize], &g.samples[k as usize + 1]);
            let p = |s: &Sample, i: usize| origin[i] + f64::from(s.pos[i]);
            let lo = |i| (p(a, i).min(p(b, i)) / size).floor() as i64;
            let hi = |i| (p(a, i).max(p(b, i)) / size).floor() as i64;
            for y in lo(1)..=hi(1) {
                for x in lo(0)..=hi(0) {
                    grid.push(([x, y], w.id, k));
                }
            }
        }
    }
    grid.sort_unstable();
    let mut nodes: BTreeMap<(u32, bool), Vec<(u32, bool)>> = BTreeMap::new();
    let arm_sets = (g.junctions.iter().map(|j| j.arms.iter().collect::<Vec<_>>()))
        .chain(g.joints.iter().map(|j| j.arms.iter().collect()))
        .chain(g.ends.iter().map(|e| vec![&e.arm]));
    for arms in arm_sets {
        for a in &arms {
            let others = arms.iter().filter(|b| !std::ptr::eq(**b, *a)).map(|b| (b.way, b.dir > 0)).collect();
            nodes.insert((a.way, a.dir > 0), others);
        }
    }
    let lengths = g.ways.iter().map(|w| between(&g.parts, &w.from, &w.to)).collect();
    let (ladders, levels, counts) = {
        let ctx = Ctx { g: &*g, input, rules, s, origin, grid, size, nodes, lengths };
        let ladders: Vec<Option<Ladder>> = g.crossings.iter().map(|c| ladder(&ctx, c)).collect();
        let mut levels = Levels { open_max_m: s.open_max_m, deck_min_m: s.deck_min_m, ..Levels::default() };
        states(&ctx, &ladders, &mut levels);
        sheets(&ctx, &mut levels);
        let counts = count(&ctx, &ladders, &levels);
        (ladders, levels, counts)
    };
    for (c, l) in g.crossings.iter_mut().zip(ladders) {
        c.ladder = l;
    }
    g.levels = levels;
    g.stats.levels = counts;
}

/// The way of a crossing's station.
fn way_of(g: &WayGraph, st: &Station) -> Option<u32> {
    let inside = |w: &&Way| w.part == st.part && key(&w.from) <= key(st) && key(st) <= key(&w.to);
    g.ways.iter().find(inside).map(|w| w.id)
}

/// The deck depth of a bridge way's tile tag, else the thinnest deck of its class.
fn deck_depth(ctx: &Ctx, w: u32) -> f64 {
    let Rule::Way(s, _) = ctx.rule(w) else { return 0.0 };
    let tag = ctx.input.features[ctx.part(w).feature.0 as usize].tags.get("deck");
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
    let (au, al) = (ctx.at_station(c.over), ctx.at_station(c.under));
    let p = [ctx.origin[0] + f64::from(c.pos[0]), ctx.origin[1] + f64::from(c.pos[1])];
    let mpu = ctx.mpu(p);
    let sin = cross(au.tangent, al.tangent).abs().max(1e-9);
    let along = |h: f64| (h / sin).min(PLACE_M) / mpu;
    let (hu, hl) = (ctx.half(upper, au.widen), ctx.half(lower, al.widen));
    let mut r = f64::INFINITY;
    for (a, b) in [(0.0, 0.0), (1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)] {
        let q = add(add(p, au.tangent, a * along(hl)), al.tangent, b * along(hu));
        let (Some(fu), Some(fl)) = (ctx.foot(upper, q), ctx.foot(lower, q)) else { return None };
        r = r.min(ctx.surface(&fu, q) - ctx.surface(&fl, q));
    }
    let Rule::Way(su, _) = ctx.rule(upper) else { return None };
    let (full, least) = match ctx.rule(lower) {
        Rule::Water(w) => (w.clearance_m + deck_depth(ctx, upper), w.clearance_least_m + su.deck_least_m),
        Rule::Way(sl, _) if ctx.tunnel(lower) => (sl.bore_m + ctx.s.cover_m, sl.bore_least_m + sl.roof_least_m),
        Rule::Way(sl, _) => (sl.clearance_m + deck_depth(ctx, upper), sl.clearance_least_m + su.deck_least_m),
    };
    let snap = match ctx.rule(lower) {
        Rule::Way(sl, _) => su.snap_height_m.min(sl.snap_height_m),
        Rule::Water(_) => su.snap_height_m,
    };
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
    let tagged = ctx.tagged(upper) || ctx.tagged(lower);
    if tagged || ctx.at_station(c.over).z != ctx.at_station(c.under).z {
        return lower;
    }
    let owner = |st: &Station| ctx.g.parts[st.part as usize].spans[st.span as usize].owner;
    if owner(&c.over) > owner(&c.under) {
        upper
    } else {
        lower
    }
}

/// Per way that has a state: its state at each sample, and the stations of its changes.
struct Track {
    way: u32,
    m: Vec<f64>,
    state: Vec<State>,
    /// A at the start and at the end, metres along, with their clearance; DECK ways only.
    a: [Option<(f64, f64)>; 2],
    tag_m: Vec<f64>,
    margin: Vec<f64>,
    deck_len: f64,
}

fn states(ctx: &Ctx, ladders: &[Option<Ladder>], out: &mut Levels) {
    let g = ctx.g;
    // The step-3 crossings by the way that ends: (crossing, the way that stays).
    let mut ended: BTreeMap<u32, Vec<(usize, u32)>> = BTreeMap::new();
    let mut culvert: Vec<bool> = vec![false; g.ways.len()];
    for (i, l) in ladders.iter().enumerate() {
        let Some(l) = l else { continue };
        if matches!(l.step, Step::Closed | Step::Inverted) {
            match l.ends {
                Some(e) => ended.entry(e).or_default().push((i, if e == l.upper { l.lower } else { l.upper })),
                None => culvert[l.upper as usize] = true,
            }
        }
    }
    let ways: Vec<u32> = (0..g.ways.len() as u32).filter(|&w| ctx.tagged(w) || ended.contains_key(&w)).collect();
    let mut tracks: Vec<Track> = Vec::new();
    // Deck ways that are not DECK: on step 3 under it, over a culvert, or too short between A and A.
    let mut ground: Vec<bool> = (0..g.ways.len() as u32)
        .map(|w| !ctx.deck_tagged(w) || culvert[w as usize] || ended.contains_key(&w))
        .collect();
    for _ in 0..4 {
        let mut changed = false;
        for &w in &ways {
            if !ctx.deck_tagged(w) || ground[w as usize] {
                continue;
            }
            let (a, len) = deck_ends(ctx, w, &ground);
            let _ = a;
            if len < ctx.s.deck_min_m {
                ground[w as usize] = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    for &w in &ways {
        let samples = ctx.samples(w);
        let st0 = g.ways[w as usize].from;
        let mut m = Vec::with_capacity(samples.len());
        let (mut last, mut acc) = (st0, 0.0);
        for s in samples {
            let st = ctx.station(w, s);
            acc += between(&g.parts, &last, &st);
            last = st;
            m.push(acc);
        }
        let len = ctx.lengths[w as usize];
        let tunnel = ctx.tunnel(w);
        let same: &dyn Fn(u32) -> bool = if tunnel { &|o| ctx.tunnel(o) } else { &|o| ctx.deck_tagged(o) };
        let limit = ctx.s.open_max_m.max(ctx.s.deck_min_m);
        let (back, ahead) = (ctx.onward(w, true, limit, same, 0), ctx.onward(w, false, limit, same, 0));
        let tag_m: Vec<f64> = m.iter().map(|&x| (x + back).min(len - x + ahead)).collect();
        let mut t = Track {
            way: w,
            state: vec![State::Open; m.len()],
            margin: vec![0.0; m.len()],
            a: [None, None],
            tag_m,
            deck_len: 0.0,
            m,
        };
        if tunnel {
            for (i, sample) in samples.iter().enumerate() {
                let a = ctx.at_station(ctx.station(w, sample));
                t.state[i] = ctx.tunnel_state(w, &a, t.tag_m[i]);
                t.margin[i] = ctx.bore(w, &a).0;
            }
            // A tunnel never becomes an open way between two bores.
            let bores: Vec<usize> = (0..t.state.len()).filter(|&i| t.state[i] == State::Bore).collect();
            if let (Some(&first), Some(&last)) = (bores.first(), bores.last()) {
                for s in &mut t.state[first..last] {
                    if *s != State::Bore {
                        *s = State::Hidden;
                    }
                }
            }
        } else if ctx.deck_tagged(w) && !ground[w as usize] {
            let (a, len) = deck_ends(ctx, w, &ground);
            let lo = a[0].map_or(0.0, |x| x.0);
            let hi = a[1].map_or(t.m.last().copied().unwrap_or(0.0), |x| x.0);
            for i in 0..t.m.len() {
                if t.m[i] >= lo && t.m[i] <= hi {
                    t.state[i] = State::Deck;
                }
            }
            t.a = a;
            t.deck_len = len;
        }
        // Step 3: not drawn over the body of the way that stays.
        for &(i, stay) in ended.get(&w).into_iter().flatten() {
            let c = &g.crossings[i];
            let st = if way_of(g, &c.over) == Some(w) { c.over } else { c.under };
            let m0 = ctx.metres(w, &st);
            let k0 = t.m.partition_point(|&x| x < m0);
            let over = |k: usize| ctx.in_body(stay, ctx.global(&samples[k]));
            for k in (k0..samples.len()).take_while(|&k| over(k)) {
                t.state[k] = State::Hidden;
            }
            for k in (0..k0).rev().take_while(|&k| over(k)) {
                t.state[k] = State::Hidden;
            }
        }
        tracks.push(t);
    }
    for t in &tracks {
        runs(ctx, t, &ended, out);
    }
}

/// The A lines of a DECK way at its tag ends, (metres along, clearance), and the metres between them
/// read on through deck joints up to `deck_min_m`.
fn deck_ends(ctx: &Ctx, w: u32, ground: &[bool]) -> ([Option<(f64, f64)>; 2], f64) {
    let len = ctx.lengths[w as usize];
    let deck = |o: u32| ctx.deck_tagged(o) && !ground[o as usize];
    let mut a = [None, None];
    for (i, start) in [(0, true), (1, false)] {
        let Some(others) = ctx.nodes.get(&(w, start)) else { continue };
        if others.iter().any(|(o, _)| deck(*o)) {
            continue;
        }
        let arms: Vec<u32> = others.iter().map(|(o, _)| *o).filter(|&o| !ctx.water(o)).collect();
        a[i] = Some(place_a(ctx, w, start, &arms, len));
    }
    let lo = a[0].map_or(0.0, |x| x.0);
    let hi = a[1].map_or(len, |x| x.0);
    let mut between_m = hi - lo;
    for (i, start) in [(0, true), (1, false)] {
        if a[i].is_none() {
            between_m += ctx.onward(w, start, ctx.s.deck_min_m, &deck, 0).min(ctx.s.deck_min_m);
        }
    }
    (a, between_m)
}

/// How far the deck stroke at metres m stands clear of the paved strokes of the arms, metres.
fn clear(ctx: &Ctx, w: u32, m: f64, arms: &[u32]) -> f64 {
    let a = ctx.at(w, m);
    let mpu = ctx.mpu(a.p);
    let h = (ctx.half(w, a.widen) + ctx.s.edge_band_m) / mpu;
    let n = [-a.tangent[1], a.tangent[0]];
    let (p1, p2) = (add(a.p, n, h), add(a.p, n, -h));
    let mut best = f64::INFINITY;
    for (o, k) in ctx.ways_near(a.p, h * mpu + HALF_MAX_M + BODY_M) {
        if !arms.contains(&o) {
            continue;
        }
        let (sa, sb) = (&ctx.g.samples[k as usize], &ctx.g.samples[k as usize + 1]);
        let d = segs_dist(p1, p2, ctx.global(sa), ctx.global(sb)) * mpu;
        best = best.min(d - ctx.half(o, sa.widen_m.max(sb.widen_m)));
    }
    best
}

/// An A line: from the tag end inward to the first sample where the deck stroke is clear of every
/// ground arm, then back in steps of `A_STEP_M` and bisected. At the far end of the way where none is.
fn place_a(ctx: &Ctx, w: u32, start: bool, arms: &[u32], len: f64) -> (f64, f64) {
    let at = |d: f64| if start { d } else { len - d };
    let ok = |d: f64| clear(ctx, w, at(d), arms) > 0.0;
    if ok(0.0) {
        return (at(0.0), clear(ctx, w, at(0.0), arms));
    }
    // The samples' metres from this end, inward.
    let samples = ctx.samples(w);
    let from = ctx.g.ways[w as usize].from;
    let mut ds: Vec<f64> = samples.iter().map(|s| between(&ctx.g.parts, &from, &ctx.station(w, s))).collect();
    if !start {
        ds = ds.iter().rev().map(|m| len - m).collect();
    }
    let first = ds.iter().copied().find(|&d| d > 0.0 && ok(d));
    let Some(hit) = first else { return (at(len), 0.0) };
    let mut d = ds.iter().copied().filter(|&x| x < hit).fold(0.0, f64::max);
    while d < hit {
        let next = (d + A_STEP_M).min(hit);
        if ok(next) {
            let (mut lo, mut hi) = (d, next);
            while hi - lo > PLACE_TO_M {
                let mid = (lo + hi) / 2.0;
                if ok(mid) {
                    hi = mid;
                } else {
                    lo = mid;
                }
            }
            return (at(hi), clear(ctx, w, at(hi), arms));
        }
        d = next;
    }
    (at(len), 0.0)
}

/// The runs and end lines of a track: a change between two samples lies where the state of the
/// side before ends, bisected; an A lies where `deck_ends` put it.
fn runs(ctx: &Ctx, t: &Track, ended: &BTreeMap<u32, Vec<(usize, u32)>>, out: &mut Levels) {
    let g = ctx.g;
    let w = t.way;
    let samples = ctx.samples(w);
    let tunnel = ctx.tunnel(w);
    let stays: Vec<u32> = ended.get(&w).into_iter().flatten().map(|&(_, s)| s).collect();
    let state_at = |m: f64| -> State {
        let a = ctx.at(w, m);
        if stays.iter().any(|&s| ctx.in_body(s, a.p)) {
            return State::Hidden;
        }
        if tunnel {
            let tag = t.m.iter().zip(&t.tag_m).map(|(x, d)| d + (x - m).abs()).fold(f64::INFINITY, f64::min);
            ctx.tunnel_state(w, &a, tag)
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
        let (to, line) = if i == t.state.len() {
            (g.ways[w as usize].to, None)
        } else {
            let (m0, m1, next) = (t.m[i - 1], t.m[i], t.state[i]);
            let a_at = t.a.iter().flatten().map(|x| x.0).find(|&x| x > m0 && x <= m1);
            let m = match (state, next) {
                (State::Open, State::Deck) | (State::Deck, State::Open) if a_at.is_some() => a_at.unwrap(),
                _ => {
                    // The change is where the state that tells the two sides apart begins or ends.
                    let tell = if state == State::Bore || next == State::Bore {
                        State::Bore
                    } else if state == State::Deck || next == State::Deck {
                        State::Deck
                    } else {
                        State::Hidden
                    };
                    let member = |s: State| match tell {
                        State::Deck => s != State::Hidden,
                        _ => s == tell,
                    };
                    let (mut lo, mut hi) = (m0, m1);
                    while hi - lo > PLACE_TO_M {
                        let mid = (lo + hi) / 2.0;
                        if member(state_at(mid)) == member(state) {
                            lo = mid;
                        } else {
                            hi = mid;
                        }
                    }
                    hi
                }
            };
            let kind = match (state, next) {
                (State::Hidden, _) | (_, State::Hidden) => EndKind::Closed,
                (State::Deck, _) | (_, State::Deck) => EndKind::A,
                _ => EndKind::Portal,
            };
            let st = ctx.at(w, m).st;
            (st, Some((kind, st, state.max(next))))
        };
        let range = first..i;
        let margin = match state {
            State::Bore => t.margin[range.clone()].iter().copied().fold(f64::INFINITY, f64::min),
            State::Deck => t.deck_len,
            _ => 0.0,
        };
        let tag_m = t.tag_m[range].iter().copied().fold(f64::INFINITY, f64::min);
        out.runs.push(Run { way: w, from, to, state, sheet: None, margin_m: margin, tag_m });
        if let Some((kind, st, structure)) = line {
            let clear_m = t.a.iter().flatten().find(|x| ctx.at(w, x.0).st == st).map_or(0.0, |x| x.1);
            out.end_lines.push(line_at(ctx, w, st, kind, structure, clear_m));
        }
        from = to;
        first = i;
    }
    // A change at a tag end: the way's end state against the ground way beyond it.
    for (start, k) in [(true, 0), (false, samples.len() - 1)] {
        let state = t.state[k];
        let Some(others) = ctx.nodes.get(&(w, start)) else { continue };
        let same = |o: u32| if tunnel { ctx.tunnel(o) } else { ctx.deck_tagged(o) };
        if state == State::Open || others.iter().any(|(o, _)| same(*o)) {
            continue;
        }
        let end_m = if start { 0.0 } else { ctx.lengths[w as usize] };
        let at_a = t.a[usize::from(!start)].is_some_and(|x| (x.0 - end_m).abs() < 1e-9);
        if state == State::Deck && !at_a {
            continue;
        }
        let st = if start { g.ways[w as usize].from } else { g.ways[w as usize].to };
        let kind = match state {
            State::Deck => EndKind::A,
            State::Bore => EndKind::Portal,
            _ => EndKind::Closed,
        };
        let clear_m = t.a[usize::from(!start)].map_or(0.0, |x| x.1);
        out.end_lines.push(line_at(ctx, w, st, kind, state, clear_m));
    }
}

/// The end line of one way at a station: across the stroke of the structure side.
fn line_at(ctx: &Ctx, w: u32, st: Station, kind: EndKind, structure: State, clear_m: f64) -> EndLine {
    let a = ctx.at_station(st);
    let band = if matches!(structure, State::Deck | State::Bore) { ctx.s.edge_band_m } else { 0.0 };
    let h = (ctx.half(w, a.widen) + band) / ctx.mpu(a.p);
    let n = [-a.tangent[1], a.tangent[0]];
    let local = |p: [f64; 2]| [(libm::round(p[0]) - ctx.origin[0]) as i32, (libm::round(p[1]) - ctx.origin[1]) as i32];
    EndLine { kind, ways: vec![(w, st)], ends: [local(add(a.p, n, h)), local(add(a.p, n, -h))], clear_m }
}

/// The sheets: DECK (or BORE) runs whose paved strokes overlap while their surfaces differ by less
/// than the least room of the pair, or whose strokes only touch within `snap_height_m`; then the
/// storeys by height among sheets that overlap in plan; then the merged end lines.
fn sheets(ctx: &Ctx, out: &mut Levels) {
    let pieces: Vec<usize> =
        (0..out.runs.len()).filter(|&i| matches!(out.runs[i].state, State::Deck | State::Bore)).collect();
    // The samples of each piece: (bucket, piece, sample index).
    let size = super::BUCKET_M / metres_per_unit_at(ctx.origin[1]);
    let mut grid: Vec<([i64; 2], usize, usize)> = Vec::new();
    for (p, &i) in pieces.iter().enumerate() {
        let r = &out.runs[i];
        for (k, s) in ctx.samples(r.way).iter().enumerate() {
            let st = ctx.station(r.way, s);
            if key(&st) >= key(&r.from) && key(&st) <= key(&r.to) {
                let q = ctx.global(s);
                grid.push(([(q[0] / size).floor() as i64, (q[1] / size).floor() as i64], p, k));
            }
        }
    }
    grid.sort_unstable();
    let mut root: Vec<usize> = (0..pieces.len()).collect();
    fn find(root: &mut [usize], mut i: usize) -> usize {
        while root[i] != i {
            root[i] = root[root[i]];
            i = root[i];
        }
        i
    }
    // (lower piece, upper piece) of pieces that overlap in plan a storey apart.
    let mut above: Vec<(usize, usize)> = Vec::new();
    let reach = 2.0 * (HALF_MAX_M + ctx.s.edge_band_m);
    for &(cell, p, k) in &grid {
        let rp = &out.runs[pieces[p]];
        let sp = &ctx.samples(rp.way)[k];
        let q = ctx.global(sp);
        let mpu = ctx.mpu(q);
        let r = (reach / mpu / size).ceil() as i64;
        for y in cell[1] - r..=cell[1] + r {
            for x in cell[0] - r..=cell[0] + r {
                let from = grid.partition_point(|e| e.0 < [x, y]);
                for &(_, p2, k2) in grid[from..].iter().take_while(|e| e.0 == [x, y]) {
                    if p2 <= p {
                        continue;
                    }
                    let r2 = &out.runs[pieces[p2]];
                    if r2.state != rp.state || r2.way == rp.way {
                        continue;
                    }
                    let s2 = &ctx.samples(r2.way)[k2];
                    let d = dot(sub(ctx.global(s2), q), sub(ctx.global(s2), q)).sqrt() * mpu;
                    let (h1, h2) = (ctx.half(rp.way, sp.widen_m), ctx.half(r2.way, s2.widen_m));
                    let band = 2.0 * ctx.s.edge_band_m;
                    if d > h1 + h2 + band {
                        continue;
                    }
                    let (lo, hi) = if sp.z_m <= s2.z_m { (rp.way, r2.way) } else { (r2.way, rp.way) };
                    let (Rule::Way(sl, _), Rule::Way(sh, _)) = (ctx.rule(lo), ctx.rule(hi)) else { continue };
                    let least = if rp.state == State::Bore {
                        sl.bore_least_m + sl.roof_least_m
                    } else {
                        sl.clearance_least_m + sh.deck_least_m
                    };
                    let dz = (sp.z_m - s2.z_m).abs();
                    let snap = sl.snap_height_m.min(sh.snap_height_m);
                    let paved = d <= h1 + h2;
                    if (paved && dz < least) || (!paved && dz <= snap) {
                        let (a, b) = (find(&mut root, p), find(&mut root, p2));
                        root[a.max(b)] = a.min(b);
                    } else if paved {
                        let pair = if sp.z_m <= s2.z_m { (p, p2) } else { (p2, p) };
                        above.push(pair);
                    }
                }
            }
        }
    }
    let mut ids: BTreeMap<usize, u32> = BTreeMap::new();
    for (p, &piece) in pieces.iter().enumerate() {
        let r = find(&mut root, p);
        let next = ids.len() as u32;
        let id = *ids.entry(r).or_insert(next);
        if id as usize == out.sheets.len() {
            out.sheets.push(Sheet { runs: Vec::new(), storey: 0 });
        }
        out.sheets[id as usize].runs.push(piece as u32);
        out.runs[piece].sheet = Some(id);
    }
    let sheet_of = |p: usize| out.runs[pieces[p]].sheet.unwrap() as usize;
    let mut pairs: Vec<(usize, usize)> = above.iter().map(|&(a, b)| (sheet_of(a), sheet_of(b))).collect();
    pairs.sort_unstable();
    pairs.dedup();
    pairs.retain(|(a, b)| a != b);
    // Decks count up from the ground, bores down: the depth of each sheet in its chain.
    let mut depth = vec![1i8; out.sheets.len()];
    for _ in 0..out.sheets.len().min(8) {
        let mut changed = false;
        for &(lo, hi) in &pairs {
            let bore = out.runs[out.sheets[lo].runs[0] as usize].state == State::Bore;
            let (from, to) = if bore { (hi, lo) } else { (lo, hi) };
            if depth[to] < depth[from].saturating_add(1) {
                depth[to] = depth[from] + 1;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    for (s, d) in out.sheets.iter_mut().zip(depth) {
        let bore = out.runs[s.runs[0] as usize].state == State::Bore;
        s.storey = if bore { -d } else { d };
    }
    merge(ctx, out);
}

/// End lines of one sheet merge where their segments cross or lie within `edge_band_m`: one straight
/// line normal to the mean direction of the merged ways, at the line farthest into the sheet.
fn merge(ctx: &Ctx, out: &mut Levels) {
    let sheet_at = |line: &EndLine| -> Option<u32> {
        let (w, st) = line.ways[0];
        out.runs
            .iter()
            .filter(|r| r.way == w && r.sheet.is_some())
            .find(|r| key(&r.from) <= key(&st) && key(&st) <= key(&r.to))
            .and_then(|r| r.sheet)
    };
    let sheets: Vec<Option<u32>> = out.end_lines.iter().map(sheet_at).collect();
    let o = ctx.origin;
    let gl = |p: [i32; 2]| [o[0] + f64::from(p[0]), o[1] + f64::from(p[1])];
    let n = out.end_lines.len();
    let mut root: Vec<usize> = (0..n).collect();
    fn find(root: &mut [usize], mut i: usize) -> usize {
        while root[i] != i {
            root[i] = root[root[i]];
            i = root[i];
        }
        i
    }
    for i in 0..n {
        for j in i + 1..n {
            let (a, b) = (&out.end_lines[i], &out.end_lines[j]);
            if sheets[i].is_none() || sheets[i] != sheets[j] || a.kind != b.kind || a.kind == EndKind::Closed {
                continue;
            }
            if a.ways[0].0 == b.ways[0].0 {
                continue;
            }
            let mpu = ctx.mpu(gl(a.ends[0]));
            let d = segs_dist(gl(a.ends[0]), gl(a.ends[1]), gl(b.ends[0]), gl(b.ends[1])) * mpu;
            if d <= ctx.s.edge_band_m {
                let (x, y) = (find(&mut root, i), find(&mut root, j));
                root[x.max(y)] = x.min(y);
            }
        }
    }
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..n {
        groups.entry(find(&mut root, i)).or_default().push(i);
    }
    let mut lines = Vec::with_capacity(groups.len());
    let mut shifts: Vec<(u32, Station, Station)> = Vec::new();
    for members in groups.into_values() {
        if members.len() == 1 {
            lines.push(out.end_lines[members[0]].clone());
            continue;
        }
        // Each member's direction into the sheet: along its way toward its structure run.
        let into = |i: usize| -> ([f64; 2], [f64; 2]) {
            let (w, st) = out.end_lines[i].ways[0];
            let a = ctx.at_station(st);
            let inside = out.runs.iter().any(|r| r.way == w && r.sheet == sheets[i] && r.from == st);
            let sign = if inside { 1.0 } else { -1.0 };
            (a.p, [sign * a.tangent[0], sign * a.tangent[1]])
        };
        let dirs: Vec<([f64; 2], [f64; 2])> = members.iter().map(|&i| into(i)).collect();
        let mut mean = dirs.iter().fold([0.0, 0.0], |s, d| add(s, d.1, 1.0));
        let len = dot(mean, mean).sqrt().max(1e-12);
        mean = [mean[0] / len, mean[1] / len];
        let far = (0..dirs.len()).max_by(|&a, &b| dot(dirs[a].0, mean).total_cmp(&dot(dirs[b].0, mean))).unwrap();
        let centre = dirs[far].0;
        let n = [-mean[1], mean[0]];
        let mut ext = (f64::INFINITY, f64::NEG_INFINITY);
        let mut ways = Vec::new();
        for &i in &members {
            let l = &out.end_lines[i];
            for e in l.ends {
                let s = dot(sub(gl(e), centre), n);
                ext = (ext.0.min(s), ext.1.max(s));
            }
            // The way's station on the merged line: its own, moved in along the way to the line.
            let (w, st) = l.ways[0];
            let (p0, d0) = into(i);
            let gap = (dot(sub(centre, p0), mean) / dot(d0, mean).max(1e-6)).max(0.0);
            let m = ctx.metres(w, &st);
            let sign = if dot(d0, ctx.at_station(st).tangent) >= 0.0 { 1.0 } else { -1.0 };
            let moved = (m + sign * gap * ctx.mpu(p0)).clamp(0.0, ctx.lengths[w as usize]);
            let to = ctx.at(w, moved).st;
            ways.push((w, to));
            shifts.push((w, st, to));
        }
        let local = |p: [f64; 2]| [(libm::round(p[0]) - o[0]) as i32, (libm::round(p[1]) - o[1]) as i32];
        let clear_m = members.iter().map(|&i| out.end_lines[i].clear_m).fold(f64::INFINITY, f64::min);
        lines.push(EndLine {
            kind: out.end_lines[members[0]].kind,
            ways,
            ends: [local(add(centre, n, ext.1)), local(add(centre, n, ext.0))],
            clear_m,
        });
    }
    out.end_lines = lines;
    // A way whose own line lay farther out keeps a ground stretch up to the merged line.
    for (w, own, to) in shifts.into_iter().filter(|s| s.1 != s.2) {
        let way = &ctx.g.ways[w as usize];
        let Some(i) = out.runs.iter().position(|r| r.way == w && r.sheet.is_some() && (r.from == own || r.to == own))
        else {
            continue;
        };
        let starts = out.runs[i].from == own;
        let next = if starts { i.checked_sub(1) } else { Some(i + 1) };
        let neighbour =
            next.filter(|&j| j < out.runs.len() && out.runs[j].way == w && out.runs[j].state == State::Open);
        let tag_m = out.runs[i].tag_m;
        let open = |from, to| Run { way: w, from, to, state: State::Open, sheet: None, margin_m: 0.0, tag_m };
        if starts {
            out.runs[i].from = to;
            match neighbour {
                Some(j) => out.runs[j].to = to,
                None => out.runs.insert(i, open(way.from, to)),
            }
        } else {
            out.runs[i].to = to;
            match neighbour {
                Some(j) => out.runs[j].from = to,
                None => out.runs.insert(i + 1, open(to, way.to)),
            }
        }
    }
    for (id, sheet) in out.sheets.iter_mut().enumerate() {
        sheet.runs = (0..out.runs.len() as u32).filter(|&r| out.runs[r as usize].sheet == Some(id as u32)).collect();
    }
}

/// The counters of the levels: owned elements only, so a sum over cells counts each once.
fn count(ctx: &Ctx, ladders: &[Option<Ladder>], levels: &Levels) -> BTreeMap<String, u32> {
    let g = ctx.g;
    let cell = g.cell;
    let owned = |p: [f64; 2]| cell.contains(GlobalPoint { x: libm::round(p[0]) as i64, y: libm::round(p[1]) as i64 });
    let mut c: BTreeMap<String, u32> = BTreeMap::new();
    let mut add_to = |name: String, n: u32| *c.entry(name).or_default() += n;
    let family = |w: u32| match g.ways[w as usize].family {
        Family::Road => "road",
        Family::Path => "path",
        Family::Rail => "rail",
        Family::Water => "water",
    };
    let class = |w: u32| match ctx.rule(w) {
        Rule::Way(s, _) => s.class.to_string(),
        Rule::Water(_) => "water".to_string(),
    };
    for (x, l) in g.crossings.iter().zip(ladders) {
        if x.owned && x.kind == CrossingKind::Bore && l.is_none() {
            // Water over a tunnel way: free ground for the state rule, no ladder.
            add_to("bore under water".into(), 1);
        }
        let Some(l) = l else { continue };
        if !x.owned {
            continue;
        }
        let kind = if x.kind == CrossingKind::LevelCrossing {
            "level_crossing".to_string()
        } else if ctx.tunnel(l.lower) {
            format!("bore_{}_under_{}", family(l.lower), family(l.upper))
        } else {
            format!("deck_over_{}", family(l.lower))
        };
        add_to(format!("ladder {kind} {:?}", l.step), 1);
        add_to(format!("ladder all {:?}", l.step), 1);
        if let Some(e) = l.ends {
            let stay = if e == l.upper { l.lower } else { l.upper };
            add_to(format!("closed {} over {} ends {}", class(l.upper), class(l.lower), class(e)), 1);
            if ctx.tunnel(e) && g.ways[e as usize].family == Family::Rail {
                add_to("rail tunnel closed (a) hidden, (b) open and ends ".to_string() + &class(stay), 1);
            }
        }
    }
    for r in &levels.runs {
        let samples = ctx.samples(r.way);
        let mut metres = 0.0;
        let mut near = 0;
        for pair in samples.windows(2) {
            let (sa, sb) = (ctx.station(r.way, &pair[0]), ctx.station(r.way, &pair[1]));
            if key(&sa) < key(&r.from) || key(&sb) > key(&r.to) {
                continue;
            }
            let mid = add(ctx.global(&pair[0]), sub(ctx.global(&pair[1]), ctx.global(&pair[0])), 0.5);
            if owned(mid) {
                metres += between(&g.parts, &sa, &sb);
                if r.state == State::Bore
                    && ctx.feet(mid, HALF_MAX_M + BODY_M).iter().any(|f| {
                        f.way != r.way
                            && !ctx.water(f.way)
                            && g.ways[f.way as usize].level == Level::Ground
                            && f.d * ctx.mpu(mid) <= ctx.foot_half(f) + BODY_M
                    })
                {
                    near += 1;
                }
            }
        }
        let tagged = if ctx.tagged(r.way) { "tagged" } else { "untagged" };
        add_to(format!("metres {tagged} {:?}", r.state), libm::round(metres) as u32);
        if ctx.tunnel(r.way) && g.ways[r.way as usize].family == Family::Rail && r.state == State::Hidden {
            add_to("rail tunnel hidden metres".into(), libm::round(metres) as u32);
        }
        add_to("bore stations within a slope reach of a ground way".into(), near);
    }
    for s in &levels.sheets {
        let first = &levels.runs[s.runs[0] as usize];
        if !owned(ctx.at_station(first.from).p) {
            continue;
        }
        let mut ways: Vec<u32> = s.runs.iter().map(|&r| levels.runs[r as usize].way).collect();
        ways.sort_unstable();
        ways.dedup();
        add_to("sheets".into(), 1);
        add_to("sheets with more than one way".into(), u32::from(ways.len() > 1));
        add_to("sheets beyond storey 1".into(), u32::from(s.storey.abs() > 1));
    }
    for l in &levels.end_lines {
        let p = ctx.at_station(l.ways[0].1).p;
        if owned(p) {
            add_to(format!("end lines {:?}", l.kind), 1);
            add_to("end lines merged".into(), u32::from(l.ways.len() > 1));
        }
    }
    // T15: a DECK end that goes on as a tunnel way whose first stations lie over the terrain, and
    // the metres until the terrain rises over its floor.
    for r in levels.runs.iter().filter(|r| r.state == State::Deck) {
        let w = r.way;
        for (start, st) in [(true, g.ways[w as usize].from), (false, g.ways[w as usize].to)] {
            if (start && r.from != st) || (!start && r.to != st) {
                continue;
            }
            for &(o, o_start) in ctx.nodes.get(&(w, start)).into_iter().flatten() {
                if !ctx.tunnel(o) || !owned(ctx.at_station(st).p) {
                    continue;
                }
                let len = ctx.lengths[o as usize];
                let over = |m: f64| {
                    let a = ctx.at(o, if o_start { m } else { len - m });
                    a.z > ctx.terrain(a.p)
                };
                if !over(0.0) {
                    continue;
                }
                let mut m = 0.0;
                while m < len && over(m) {
                    m += 1.0;
                }
                let bin = match m {
                    x if x >= len => "the tunnel way ends first",
                    x if x <= 10.0 => "0-10 m",
                    x if x <= 30.0 => "10-30 m",
                    x if x <= 100.0 => "30-100 m",
                    _ => "over 100 m",
                };
                add_to(format!("t15 {bin}"), 1);
            }
        }
    }
    c
}
