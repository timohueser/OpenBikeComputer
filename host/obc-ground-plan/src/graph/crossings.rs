//! Stage 4: crossings without a shared vertex. Sample segments meet by exact orientation tests; the
//! type table gives each a type; synthetic decks split the upper piece.

use std::collections::BTreeMap;

use super::curve::{insert, point};
use super::parts::Rule;
use super::points::{advance, key, metres, nodes, Anchor, Node, NodeKind, Piece, Plan};
use super::{
    Crossing, CrossingKind, Family, GraphStats, Group, Level, Part, Sample, Source, Station, BUCKET_M, DECK_HALF_MAX_M,
    DECK_SIDE_M, WATER_BANK_M,
};
use crate::frame::{metres_per_unit, CellKey};
use crate::sections::GRAZE_DEG;

/// A chord between two neighbouring samples of a piece, in global positions.
#[derive(Clone, Debug)]
pub(crate) struct Seg {
    pub piece: usize,
    pub a: [i64; 2],
    pub b: [i64; 2],
    /// The stations at a and at b, both on a's span.
    pub sa: Station,
    pub ub: f64,
}

impl Seg {
    pub fn at(&self, t: f64) -> Station {
        Station { u: self.sa.u + t * (self.ub - self.sa.u), ..self.sa }
    }
}

/// The sample segments of the pieces in buckets of `BUCKET_M`: (bucket, segment), sorted.
pub(crate) struct Grid {
    pub size: i64,
    pub segs: Vec<Seg>,
    pub cells: Vec<([i64; 2], u32)>,
}

/// The samples of a piece.
pub(crate) fn piece_samples<'a>(samples: &'a [Sample], piece: &Piece) -> impl Iterator<Item = &'a Sample> {
    let (from, to) = (key(&piece.from), key(&piece.to));
    samples.iter().filter(move |s| (s.span, s.u) >= from && (s.span, s.u) <= to)
}

impl Grid {
    pub fn new(cell: CellKey, samples: &[Vec<Sample>], pieces: &[Piece]) -> Grid {
        let o = cell.origin();
        let size = ((BUCKET_M / metres_per_unit(o.y)) as i64).max(1);
        let mut grid = Grid { size, segs: Vec::new(), cells: Vec::new() };
        for (index, piece) in pieces.iter().enumerate() {
            let list: Vec<&Sample> = piece_samples(&samples[piece.part], piece).collect();
            for pair in list.windows(2) {
                let global = |s: &Sample| [o.x + i64::from(s.pos[0]), o.y + i64::from(s.pos[1])];
                let ub = if pair[1].span == pair[0].span { pair[1].u } else { 1.0 };
                let seg = Seg {
                    piece: index,
                    a: global(pair[0]),
                    b: global(pair[1]),
                    sa: Station { part: piece.part as u32, span: pair[0].span, u: pair[0].u },
                    ub,
                };
                let id = grid.segs.len() as u32;
                let cells = &mut grid.cells;
                cover(size, seg.a, seg.b, 0, |cell| cells.push((cell, id)));
                grid.segs.push(seg);
            }
        }
        grid.cells.sort_unstable();
        grid
    }

    /// The segments of each bucket.
    pub fn buckets(&self) -> impl Iterator<Item = &[([i64; 2], u32)]> {
        self.cells.chunk_by(|a, b| a.0 == b.0)
    }

    /// The segments whose buckets the box of a and b grown by `reach` units meets, once each, in id order.
    pub fn near(&self, a: [i64; 2], b: [i64; 2], reach: i64) -> Vec<u32> {
        let mut out: Vec<u32> = Vec::new();
        cover(self.size, a, b, reach, |cell| {
            let from = self.cells.partition_point(|c| c.0 < cell);
            out.extend(self.cells[from..].iter().take_while(|c| c.0 == cell).map(|c| c.1));
        });
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// Each bucket that the box of a and b grown by `reach` units meets.
fn cover(size: i64, a: [i64; 2], b: [i64; 2], reach: i64, mut f: impl FnMut([i64; 2])) {
    let lo = |i: usize| (a[i].min(b[i]) - reach).div_euclid(size);
    let hi = |i: usize| (a[i].max(b[i]) + reach).div_euclid(size);
    for y in lo(1)..=hi(1) {
        for x in lo(0)..=hi(0) {
            f([x, y]);
        }
    }
}

fn orient(a: [i64; 2], b: [i64; 2], c: [i64; 2]) -> i8 {
    let d = |p: i64, q: i64| i128::from(p) - i128::from(q);
    let v = d(b[0], a[0]) * d(c[1], a[1]) - d(b[1], a[1]) * d(c[0], a[0]);
    v.signum() as i8
}

/// Where the half-open segments [a, b) and [c, d) cross, exactly: the parameters on each. Collinear
/// segments do not cross.
pub(crate) fn cross(a: [i64; 2], b: [i64; 2], c: [i64; 2], d: [i64; 2]) -> Option<(f64, f64)> {
    let (o1, o2, o3, o4) = (orient(a, b, c), orient(a, b, d), orient(c, d, a), orient(c, d, b));
    if o1 == 0 && o2 == 0 {
        return None;
    }
    let first = o1 * o2 < 0 || (o1 == 0 && o2 != 0);
    let second = o3 * o4 < 0 || (o3 == 0 && o4 != 0);
    if !(first && second) {
        return None;
    }
    let r = [(b[0] - a[0]) as f64, (b[1] - a[1]) as f64];
    let s = [(d[0] - c[0]) as f64, (d[1] - c[1]) as f64];
    let q = [(c[0] - a[0]) as f64, (c[1] - a[1]) as f64];
    let rs = r[0] * s[1] - r[1] * s[0];
    Some(((q[0] * s[1] - q[1] * s[0]) / rs, (q[0] * r[1] - q[1] * r[0]) / rs))
}

/// The end of a synthetic stretch: metres along its piece, and its station.
type StretchEnd = (f64, Station);

/// One side of a crossing.
#[derive(Clone, Copy)]
struct Side<'a> {
    piece: usize,
    st: Station,
    family: Family,
    level: Level,
    z: f64,
    rule: &'a Rule,
}

/// What the table gives a crossing.
enum Outcome {
    Record(CrossingKind, Source),
    Implicit,
    /// A synthetic deck or footbridge over the lower side, and the half-width that it spans.
    Synthetic(CrossingKind, f64),
}

/// The length of a culvert chain about a station, read up to `limit` metres each way.
fn chain(parts: &[Part], plan: &Plan, at: &BTreeMap<([i64; 2], Group), &Node>, side: &Side, limit: f64) -> f64 {
    let mut total = 0.0;
    for start in [1i8, -1] {
        let (mut piece, mut m, mut dir, mut run) =
            (side.piece, metres(parts, &plan.pieces[side.piece], &side.st), start, 0.0);
        for _ in 0..64 {
            let p = &plan.pieces[piece];
            let rest = if dir > 0 { metres(parts, p, &p.to) - m } else { m };
            run += rest;
            if run > limit {
                break;
            }
            let end = if dir > 0 { p.to } else { p.from };
            let pos = super::points::position(parts, &end);
            let Some(node) = at.get(&(pos, Group::Water)).filter(|n| n.kind == NodeKind::Joint) else { break };
            let Some(other) = node.arms.iter().find(|a| a.piece != piece || a.dir == dir) else { break };
            if plan.pieces[other.piece].level != Level::Tunnel {
                break;
            }
            (piece, m, dir) = (other.piece, other.m, other.dir);
        }
        total += run;
    }
    total
}

/// The crossings of the pieces, the implicit junctions, and the pieces split at synthetic decks.
pub(crate) fn crossings(
    cell: CellKey,
    area: [i64; 4],
    parts: &[Part],
    rules: &[Rule],
    samples: &mut [Vec<Sample>],
    mut plan: Plan,
    stats: &mut GraphStats,
) -> Plan {
    let prelim = nodes(parts, &plan, area, None);
    let at: BTreeMap<([i64; 2], Group), &Node> = prelim.nodes.iter().map(|n| ((n.pos, n.group), n)).collect();
    let grid = Grid::new(cell, samples, &plan.pieces);
    let mut pairs: Vec<(u32, u32)> = Vec::new();
    for ids in grid.buckets() {
        for (i, &(_, x)) in ids.iter().enumerate() {
            pairs.extend(ids[i + 1..].iter().map(|&(_, y)| (x.min(y), x.max(y))));
        }
    }
    pairs.sort_unstable();
    pairs.dedup();
    let o = cell.origin();
    let graze = GRAZE_DEG.to_radians();
    // (position, upper side, lower side, kind, source); synthetic decks wait for their test.
    let mut found: Vec<([i64; 2], Side, Side, Outcome, f64)> = Vec::new();
    for (x, y) in pairs {
        let (s, t) = (&grid.segs[x as usize], &grid.segs[y as usize]);
        let (pa, pb) = (&plan.pieces[s.piece], &plan.pieces[t.piece]);
        if pa.part == pb.part && s.sa.span.abs_diff(t.sa.span) <= 1 {
            continue;
        }
        let Some((ta, tb)) = cross(s.a, s.b, t.a, t.b) else { continue };
        let r = [(s.b[0] - s.a[0]) as f64, (s.b[1] - s.a[1]) as f64];
        let pos = [s.a[0] + libm::round(ta * r[0]) as i64, s.a[1] + libm::round(ta * r[1]) as i64];
        if prelim.meets.get(&pos).is_some_and(|p| p.contains(&s.piece) && p.contains(&t.piece)) {
            continue;
        }
        let side = |seg: &Seg, piece: &Piece, f: f64| {
            let st = seg.at(f);
            let part = &parts[piece.part];
            Side {
                piece: seg.piece,
                st,
                family: part.family,
                level: piece.level,
                z: point(part, st.span as usize, st.u).z_m,
                rule: &rules[piece.part],
            }
        };
        let (a, b) = (side(s, pa, ta), side(t, pb, tb));
        let q = [(t.b[0] - t.a[0]) as f64, (t.b[1] - t.a[1]) as f64];
        let angle = libm::atan2((r[0] * q[1] - r[1] * q[0]).abs(), (r[0] * q[0] + r[1] * q[1]).abs());
        let water = |x: &Side| x.family == Family::Water;
        let culvert =
            |w: &Side, x: &Side| water(w) && w.level == Level::Tunnel && !water(x) && x.level == Level::Ground;
        if (a.level == Level::Tunnel || b.level == Level::Tunnel) && !culvert(&a, &b) && !culvert(&b, &a) {
            continue;
        }
        if water(&a) && water(&b) {
            stats.water_water += 1;
            continue;
        }
        if !water(&a) && !water(&b) && angle < graze {
            stats.grazing += 1;
            continue;
        }
        let outcome = if a.level == Level::Deck || b.level == Level::Deck {
            let over_a = b.level != Level::Deck || (a.level == Level::Deck && a.z >= b.z);
            let (u, l) = if over_a { (a, b) } else { (b, a) };
            (Outcome::Record(CrossingKind::Deck, Source::Tag), u, l)
        } else if water(&a) || water(&b) {
            let (w, x) = if water(&a) { (a, b) } else { (b, a) };
            let out = if w.level == Level::Tunnel {
                let limit = w.rule.culvert_max_m;
                if chain(parts, &plan, &at, &w, limit) <= limit {
                    Outcome::Record(CrossingKind::Culvert, Source::Tag)
                } else {
                    stats.buried += 1;
                    Outcome::Record(CrossingKind::Culvert, Source::Fallback)
                }
            } else if x.rule.ford {
                Outcome::Record(CrossingKind::Ford, Source::Tag)
            } else if angle < graze {
                stats.grazing_water += 1;
                Outcome::Record(CrossingKind::Stacked, Source::Fallback)
            } else {
                let width = w.rule.width_m;
                let half = width / 2.0 + WATER_BANK_M + DECK_SIDE_M;
                match x.family {
                    Family::Path if width <= w.rule.pipe_width_m => {
                        Outcome::Record(CrossingKind::PipeInStones, Source::Rule)
                    }
                    Family::Path if width <= w.rule.stones_width_m => {
                        Outcome::Record(CrossingKind::SteppingStones, Source::Rule)
                    }
                    Family::Path => Outcome::Synthetic(CrossingKind::Footbridge, half),
                    _ if width <= w.rule.culvert_width_m => Outcome::Record(CrossingKind::Culvert, Source::Rule),
                    _ => Outcome::Synthetic(CrossingKind::Deck, half),
                }
            };
            (out, x, w)
        } else {
            let (u, l) = if a.z >= b.z { (a, b) } else { (b, a) };
            let dz = u.z - l.z;
            let level = dz <= a.rule.snap_height_m.min(b.rule.snap_height_m);
            let rail = |x: &Side| x.family == Family::Rail;
            let out = if level && rail(&a) != rail(&b) {
                Outcome::Record(CrossingKind::LevelCrossing, Source::Rule)
            } else if level && !rail(&a) && !rail(&b) {
                Outcome::Implicit
            } else if dz >= l.rule.clearance_m {
                Outcome::Synthetic(CrossingKind::Deck, l.rule.outer_m + DECK_SIDE_M)
            } else {
                Outcome::Record(CrossingKind::Stacked, Source::Fallback)
            };
            (out, u, l)
        };
        let (outcome, upper, lower) = outcome;
        found.push((pos, upper, lower, outcome, angle));
    }
    found.sort_by_key(|a| a.0);
    let local = |p: [i64; 2]| [(p[0] - o.x) as i32, (p[1] - o.y) as i32];
    let record = |pos, over: Station, under: Station, kind, source, angle| Crossing {
        pos: local(pos),
        over,
        under,
        kind,
        source,
        angle_rad: angle,
        deck: None,
        owned: false,
    };
    // Synthetic stretches by piece: the metres and the stations of their ends.
    let mut stretches: BTreeMap<usize, Vec<(StretchEnd, StretchEnd)>> = BTreeMap::new();
    let mut stations: Vec<(usize, Station)> = Vec::new();
    for (pos, upper, lower, outcome, angle) in found {
        let piece = &plan.pieces[upper.piece];
        match outcome {
            Outcome::Implicit => {
                for side in [upper, lower] {
                    plan.anchors.push(Anchor { pos, at: side.st, implicit: true });
                    stations.push((plan.pieces[side.piece].part, side.st));
                }
            }
            Outcome::Record(kind, source) => {
                stations.push((piece.part, upper.st));
                stations.push((plan.pieces[lower.piece].part, lower.st));
                plan.crossings.push(record(pos, upper.st, lower.st, kind, source, angle));
            }
            Outcome::Synthetic(kind, half) => {
                let d = half / libm::sin(angle);
                let m = metres(parts, piece, &upper.st);
                let holds = prelim
                    .nodes
                    .iter()
                    .flat_map(|n| &n.arms)
                    .any(|arm| arm.piece == upper.piece && (arm.m - m).abs() <= d);
                if !d.is_finite() || d > DECK_HALF_MAX_M || holds {
                    stats.deck_fallback += 1;
                    plan.crossings.push(record(
                        pos,
                        upper.st,
                        lower.st,
                        CrossingKind::Stacked,
                        Source::Fallback,
                        angle,
                    ));
                    continue;
                }
                let length = metres(parts, piece, &piece.to);
                if m - d < 0.0 || m + d > length {
                    stats.deck_cut += 1;
                }
                let ends = ((m - d).max(0.0), (m + d).min(length));
                let at = |delta: f64| advance(parts, piece, &upper.st, delta);
                stretches.entry(upper.piece).or_default().push(((ends.0, at(-d)), (ends.1, at(d))));
                stations.push((plan.pieces[lower.piece].part, lower.st));
                plan.crossings.push(record(pos, upper.st, lower.st, kind, Source::Rule, angle));
            }
        }
    }
    // Every crossing point and implicit junction is a sample of both pieces.
    let mut moved: BTreeMap<(usize, u32, u64), Station> = BTreeMap::new();
    for (part, st) in stations {
        let taken = insert(&parts[part], &mut samples[part], st, cell);
        moved.insert((part, st.span, st.u.to_bits()), taken);
    }
    let take = |st: &Station| moved.get(&(st.part as usize, st.span, st.u.to_bits())).copied().unwrap_or(*st);
    for a in &mut plan.anchors {
        a.at = take(&a.at);
    }
    for c in &mut plan.crossings {
        (c.over, c.under) = (take(&c.over), take(&c.under));
    }
    // Split each piece at its merged stretches: the middle is a synthetic deck.
    let mut out = Vec::new();
    for (index, piece) in plan.pieces.iter().enumerate() {
        let Some(list) = stretches.get_mut(&index) else {
            out.push(piece.clone());
            continue;
        };
        list.sort_by(|a, b| a.0 .0.total_cmp(&b.0 .0));
        let mut merged: Vec<(StretchEnd, StretchEnd)> = Vec::new();
        for &(a, b) in list.iter() {
            match merged.last_mut() {
                Some(last) if a.0 <= last.1 .0 => {
                    if b.0 > last.1 .0 {
                        last.1 = b;
                    }
                }
                _ => merged.push((a, b)),
            }
        }
        let part = &parts[piece.part];
        let mut cut = |st: Station| insert(part, &mut samples[piece.part], st, cell);
        let mut from = piece.from;
        for (a, b) in merged {
            let (sa, sb) = (cut(a.1), cut(b.1));
            if key(&from) < key(&sa) {
                out.push(Piece { from, to: sa, ..piece.clone() });
            }
            if key(&sa) < key(&sb) {
                out.push(Piece { from: sa, to: sb, level: Level::Deck, synthetic: true, ..piece.clone() });
            }
            from = sb;
        }
        if key(&from) < key(&piece.to) {
            out.push(Piece { from, ..piece.clone() });
        }
    }
    plan.pieces = out;
    plan
}
