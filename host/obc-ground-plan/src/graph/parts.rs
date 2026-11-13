//! Stage 1: the parts of a cell. The owned runs of the read tiles join across tile lines, then each
//! part gets its family, class and level, its cleaning, and splits where its heights are unknown or its
//! water lies underground.

use std::collections::BTreeMap;

use super::curve::level_at;
use super::{
    folds, water_grades, water_spans, water_tangents, Family, FeatureRef, GraphStats, Level, Part, Span, Vertex,
    SPIKE_DEG,
};
use crate::frame::{metres_per_unit, GlobalPoint};
use crate::sections::{is_track, Sections, WaterSection, WaySection, WATER_KINDS};
use crate::tile::{self, Feature, Geometry};
use crate::CellInput;

const WAYS: [&str; 6] = ["main-road", "road", "path", "railway", "tunnel", "gallery"];
/// The classes of the kind `path`.
const PATH_CLASSES: [&str; 7] = ["path", "footway", "cycleway", "bridleway", "steps", "pedestrian", "via_ferrata"];
/// Spans kept past each span that may meet the work area: the cleaning of a vertex reads its two spans.
const TRIM_SPANS: usize = 2;
/// The step of `underground`'s test points along a span, metres.
const UNDERGROUND_STEP_M: f64 = 2.0;

/// The rules of a part: the cross-section of its class and whether it fords, or of its water kind.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Rule<'a> {
    Way(WaySection<'a>, bool),
    Water(WaterSection),
}

impl<'a> Rule<'a> {
    pub fn way(&self) -> &WaySection<'a> {
        match self {
            Rule::Way(s, _) => s,
            Rule::Water(_) => panic!("a way's rule of water"),
        }
    }

    pub fn water(&self) -> &WaterSection {
        match self {
            Rule::Water(w) => w,
            Rule::Way(..) => panic!("a water's rule of a way"),
        }
    }

    pub fn half_m(&self) -> f64 {
        match self {
            Rule::Way(s, _) => s.half_m,
            Rule::Water(w) => w.half_m,
        }
    }

    pub fn outer_m(&self) -> f64 {
        match self {
            Rule::Way(s, _) => s.outer_m(),
            Rule::Water(w) => w.half_m,
        }
    }

    /// A way tagged `ford`, or a `low_water_crossing`.
    pub fn ford(&self) -> bool {
        matches!(self, Rule::Way(_, true))
    }
}

/// Whether a way of `family` at a ford or at a vertex it shares with open water crosses that water as a ford: a path
/// always, another way only over water wider than `culvert_width_m`. As the bake lays it, a road, a track or a
/// railway otherwise takes the rule of an untagged crossing.
pub(crate) fn fords(family: Family, water: &WaterSection) -> bool {
    family == Family::Path || water.width_m > water.culvert_width_m
}

/// The family, rank, level and rules of a line feature, or None for another kind.
pub(crate) fn classify<'a>(feature: &Feature, sections: &'a Sections) -> Option<(Family, u8, Level, Rule<'a>)> {
    let tag = |key: &str| feature.tags.get(key).map(String::as_str);
    let kind = feature.kind.as_str();
    if feature.geometry != Geometry::Line {
        return None;
    }
    if WATER_KINDS.contains(&kind) {
        let w = sections.water(kind, &feature.tags);
        let level = if tag("tunnel").is_some_and(|t| t != "no") { Level::Tunnel } else { Level::Ground };
        return Some((Family::Water, w.rank as u8, level, Rule::Water(w)));
    }
    if !WAYS.contains(&kind) {
        return None;
    }
    let s = sections.way(kind, &feature.tags, false);
    let family = if is_track(&feature.tags) {
        Family::Rail
    } else if PATH_CLASSES.contains(&s.class) {
        Family::Path
    } else {
        Family::Road
    };
    let level = if kind == "tunnel" {
        Level::Tunnel
    } else if tag("deck").is_some() {
        Level::Deck
    } else {
        Level::Ground
    };
    let ford = tag("ford").is_some_and(|f| f != "no") || tag("bridge") == Some("low_water_crossing");
    Some((family, s.rank as u8, level, Rule::Way(s, ford)))
}

/// A vertex before cleaning: its height can be unknown.
#[derive(Clone, Debug)]
struct Raw {
    v: Vertex,
    z: Option<f64>,
    /// The spans at this vertex are chords: a spike, or a corner of water.
    corner: bool,
}

/// A water span's shape (`water_spans`): None for its chord, else its end tangents and handle.
type WaterShape = Option<([f64; 2], f64)>;

/// A joined part before cleaning: its vertices, per span the stroke metres at its start, whether it
/// has no alignment, its owner and its water shape, and the tile's two vertices before and after it with
/// their heights, in the chain's order, where the tile holds them; the tiles of its first and its last run.
struct Chain {
    feature: usize,
    backward: bool,
    tiles: [(u32, u32); 2],
    vertices: Vec<Raw>,
    spans: Vec<(f64, bool, [u32; 3], WaterShape)>,
    before: Vec<([i64; 2], Option<f64>)>,
    after: Vec<([i64; 2], Option<f64>)>,
}

/// The vertices of a tile part in the graph's form; a part with no alignment takes its chords.
fn tile_vertices(part: &tile::Part) -> Vec<Raw> {
    let n = part.points.len();
    (0..n)
        .map(|i| {
            let pos = [part.points[i].x, part.points[i].y];
            let z = part.heights.as_ref().and_then(|h| h[i]);
            let v = match &part.plan {
                Some(p) => Vertex {
                    pos,
                    z_m: 0.0,
                    tangent: p.tangent[i],
                    grade: p.grade[i],
                    bank: p.bank[i],
                    widen_m: p.widen[i],
                    walk: p.walk[i],
                    side: p.side[i],
                },
                None => {
                    let (a, b) = if i + 1 < n { (i, i + 1) } else { (i - 1, i) };
                    let (pa, pb) = (part.points[a], part.points[b]);
                    let tangent = libm::atan2((pb.y - pa.y) as f64, (pb.x - pa.x) as f64);
                    Vertex { pos, z_m: 0.0, tangent, grade: 0.0, bank: 0.0, widen_m: 0.0, walk: 0, side: 0 }
                }
            };
            Raw { v, z, corner: false }
        })
        .collect()
}

/// The stroke metres at each vertex of a tile part: from its `along_m`, or from 0 with no alignment.
fn tile_along(part: &tile::Part, vertices: &[Raw]) -> Vec<f64> {
    let (start, sign) = match &part.plan {
        Some(p) => (p.along_m, if p.backward { -1.0 } else { 1.0 }),
        None => (0.0, 1.0),
    };
    let chord = part.plan.is_none();
    let mut along = vec![start];
    for pair in vertices.windows(2) {
        let len = super::curve::length(&pair[0].v, &pair[1].v, chord);
        along.push(along.last().unwrap() + sign * len);
    }
    along
}

/// An owned run: vertices `from..=to` of part `part` of feature `feature`.
#[derive(Clone, Copy)]
struct Run {
    feature: usize,
    part: usize,
    from: usize,
    to: usize,
}

fn runs(input: &CellInput, line: &[bool]) -> Vec<Run> {
    let mut out = Vec::new();
    for (f, feature) in input.features.iter().enumerate().filter(|(f, _)| line[*f]) {
        for (p, part) in feature.parts.iter().enumerate() {
            let mut start = None;
            for i in 0..=part.own.len() {
                match (start, part.own.get(i).copied().unwrap_or(false)) {
                    (None, true) => start = Some(i),
                    (Some(s), false) => {
                        out.push(Run { feature: f, part: p, from: s, to: i });
                        start = None;
                    }
                    _ => {}
                }
            }
        }
    }
    out
}

fn key(p: GlobalPoint) -> [i64; 2] {
    [p.x, p.y]
}

/// Join the owned runs: a run whose part goes on with an unowned segment takes the run of another
/// tile, of a feature of the same kind and tags, that starts with that segment.
fn chains(input: &CellInput, line: &[bool]) -> Vec<Chain> {
    let runs = runs(input, line);
    let part = |r: &Run| &input.features[r.feature].parts[r.part];
    let mut starts: BTreeMap<[[i64; 2]; 2], Vec<usize>> = BTreeMap::new();
    for (i, r) in runs.iter().enumerate() {
        let points = &part(r).points;
        starts.entry([key(points[r.from]), key(points[r.from + 1])]).or_default().push(i);
    }
    let mut next = vec![None; runs.len()];
    let mut taken = vec![false; runs.len()];
    for (i, r) in runs.iter().enumerate() {
        let points = &part(r).points;
        if r.to + 1 >= points.len() {
            continue;
        }
        let f = &input.features[r.feature];
        let candidates = starts.get(&[key(points[r.to]), key(points[r.to + 1])]).into_iter().flatten();
        let found = candidates.copied().find(|&j| {
            let (g, other) = (&input.features[runs[j].feature], part(&runs[j]));
            !taken[j] && other.tile != part(r).tile && g.kind == f.kind && g.tags == f.tags
        });
        if let Some(j) = found {
            next[i] = Some(j);
            taken[j] = true;
        }
    }
    let mut seen = vec![false; runs.len()];
    let mut out = Vec::new();
    let order: Vec<usize> = (0..runs.len()).filter(|&i| !taken[i]).chain(0..runs.len()).collect();
    for first in order {
        if seen[first] {
            continue;
        }
        let r0 = runs[first];
        let past = |p: &tile::Part, i: usize| (key(p.points[i]), p.heights.as_ref().and_then(|h| h[i]));
        let before = (r0.from.saturating_sub(2)..r0.from).map(|i| past(part(&r0), i)).collect();
        let mut chain = Chain {
            feature: r0.feature,
            backward: false,
            tiles: [part(&r0).tile; 2],
            vertices: Vec::new(),
            spans: Vec::new(),
            before,
            after: Vec::new(),
        };
        let mut at = Some(first);
        while let Some(i) = at.filter(|&i| !seen[i]) {
            seen[i] = true;
            let r = runs[i];
            let p = part(&r);
            chain.backward = p.plan.as_ref().is_some_and(|plan| plan.backward);
            chain.tiles[1] = p.tile;
            let vertices = tile_vertices(p);
            let along = tile_along(p, &vertices);
            let skip = usize::from(!chain.vertices.is_empty());
            chain.vertices.extend(vertices[r.from + skip..=r.to].iter().cloned());
            let owner = [p.tile.0, p.tile.1, input.features[r.feature].index];
            chain.spans.extend((r.from..r.to).map(|k| (along[k], p.plan.is_none(), owner, None)));
            chain.after = (r.to + 1..(r.to + 3).min(p.points.len())).map(|i| past(p, i)).collect();
            at = next[i];
        }
        out.push(chain);
    }
    out
}

/// The stretches of a chain within `TRIM_SPANS` spans of a span that may meet the square: the box of
/// its chord grown by a third of the chord holds its curve. The rest decides nothing in the cell.
fn trim(chain: Chain, [x0, y0, x1, y1]: [i64; 4]) -> Vec<Chain> {
    let n = chain.spans.len();
    let near: Vec<bool> = (0..n)
        .map(|k| {
            let (a, b) = (chain.vertices[k].v.pos, chain.vertices[k + 1].v.pos);
            let d = [(b[0] - a[0]) as f64, (b[1] - a[1]) as f64];
            let grow = ((d[0] * d[0] + d[1] * d[1]).sqrt() / 3.0).ceil() as i64 + 1;
            a[0].max(b[0]) + grow >= x0
                && a[0].min(b[0]) - grow <= x1
                && a[1].max(b[1]) + grow >= y0
                && a[1].min(b[1]) - grow <= y1
        })
        .collect();
    let keep: Vec<bool> =
        (0..n).map(|k| near[k.saturating_sub(TRIM_SPANS)..(k + TRIM_SPANS + 1).min(n)].iter().any(|&m| m)).collect();
    let mut out = Vec::new();
    let mut k = 0;
    while k < n {
        if !keep[k] {
            k += 1;
            continue;
        }
        let start = k;
        while k < n && keep[k] {
            k += 1;
        }
        let before: Vec<([i64; 2], Option<f64>)> =
            chain.before.iter().copied().chain(chain.vertices[..start].iter().map(|r| (r.v.pos, r.z))).collect();
        out.push(Chain {
            feature: chain.feature,
            backward: chain.backward,
            tiles: chain.tiles,
            vertices: chain.vertices[start..=k].to_vec(),
            spans: chain.spans[start..k].to_vec(),
            before: before[before.len().saturating_sub(2)..].to_vec(),
            after: chain.vertices[k + 1..]
                .iter()
                .map(|r| (r.v.pos, r.z))
                .chain(chain.after.iter().copied())
                .take(2)
                .collect(),
        });
    }
    out
}

/// The turn in degrees of the chords from a through b to c.
fn turn_deg(a: [i64; 2], b: [i64; 2], c: [i64; 2]) -> f64 {
    let (u, v) = ([(b[0] - a[0]) as f64, (b[1] - a[1]) as f64], [(c[0] - b[0]) as f64, (c[1] - b[1]) as f64]);
    libm::atan2((u[0] * v[1] - u[1] * v[0]).abs(), u[0] * v[0] + u[1] * v[1]).to_degrees()
}

/// The neighbours of vertex i on its chain: the tile's vertices past the chain's ends included.
fn neighbours(chain: &Chain, i: usize) -> (Option<[i64; 2]>, Option<[i64; 2]>) {
    let (n, here) = (chain.vertices.len(), chain.vertices[i].v.pos);
    let prev = if i > 0 { Some(chain.vertices[i - 1].v.pos) } else { chain.before.last().map(|p| p.0) };
    let next = if i + 1 < n { Some(chain.vertices[i + 1].v.pos) } else { chain.after.first().map(|p| p.0) };
    (prev.filter(|&q| q != here), next.filter(|&q| q != here))
}

/// The tile's two vertices past an end of a water chain on the one other water line that ends there, in
/// the chain's order (`at_start`: before its first vertex, else after its last): where the map splits a
/// stream into two ways, the curve and the grades read on through the shared vertex. None where no other
/// line or more than one ends there (a confluence). Only the tile of the chain's end run counts, so every
/// cell decides alike.
fn through(input: &CellInput, water: &[bool], chain: &Chain, at_start: bool) -> Option<Vec<([i64; 2], Option<f64>)>> {
    let n = chain.vertices.len();
    let tile = chain.tiles[usize::from(!at_start)];
    let (end, own) = if at_start {
        (chain.vertices[0].v.pos, chain.vertices.get(1).map(|r| r.v.pos))
    } else {
        (chain.vertices[n - 1].v.pos, n.checked_sub(2).map(|i| chain.vertices[i].v.pos))
    };
    let mut found: Vec<Vec<([i64; 2], Option<f64>)>> = Vec::new();
    let parts = (input.features.iter().enumerate()).filter(|(f, _)| water[*f]).flat_map(|(_, f)| &f.parts);
    let parts = parts.filter(|p| p.tile == tile);
    for part in parts {
        let m = part.points.len();
        let at = |i: usize| (key(part.points[i]), part.heights.as_ref().and_then(|h| h[i]));
        let away: Vec<usize> = if m >= 2 && key(part.points[0]) == end {
            (1..m.min(3)).collect()
        } else if m >= 2 && key(part.points[m - 1]) == end {
            (m.saturating_sub(3)..m - 1).rev().collect()
        } else {
            continue;
        };
        let next: Vec<_> = away.into_iter().map(at).collect();
        // A copy of the chain's own line, or a second copy of a line already found.
        if Some(next[0].0) == own || found.iter().any(|f| f[0].0 == next[0].0) {
            continue;
        }
        found.push(next);
    }
    let mut next = found.pop().filter(|_| found.is_empty())?;
    if at_start {
        next.reverse();
    }
    Some(next)
}

/// The tangents, corners, span shapes and grades of water (`water_tangents`, `water_spans`,
/// `water_grades`). A chain end that is no end of the way reads the tile's two vertices past it, so every
/// cell that samples a span gives it one curve and one profile: a span reads the spans beside it.
fn curve_water(chain: &mut Chain, turn_max_deg: f64, off_max_m: f64) {
    let all: Vec<([i64; 2], Option<f64>)> = (chain.before.iter().copied())
        .chain(chain.vertices.iter().map(|r| (r.v.pos, r.z)))
        .chain(chain.after.iter().copied())
        .collect();
    let points: Vec<[f64; 2]> = all.iter().map(|p| p.0.map(|c| c as f64)).collect();
    let skip = chain.before.len();
    let mut tangents = water_tangents(&points, turn_max_deg);
    for (r, (tangent, corner)) in chain.vertices.iter_mut().zip(&mut tangents[skip..]) {
        r.v.tangent = *tangent;
        r.corner |= *corner;
        *corner = r.corner;
    }
    let metres = |a: [f64; 2], b: [f64; 2]| metres_per_unit(((a[1] + b[1]) / 2.0).floor() as i64);
    let shapes = water_spans(&points, &tangents, metres, off_max_m);
    for (span, shape) in chain.spans.iter_mut().zip(&shapes[skip..]) {
        span.3 = *shape;
    }
    let z: Vec<Option<f64>> = all.iter().map(|p| p.1).collect();
    for (r, grade) in chain.vertices.iter_mut().zip(&water_grades(&points, &z, metres)[skip..]) {
        r.v.grade = *grade;
    }
}

/// A span of water: its chord, or its curve with the end tangents and handle of `water_spans`.
fn water_span(a: &Vertex, b: &Vertex, along0_m: f64, owner: [u32; 3], shape: WaterShape) -> Span {
    let span = match shape {
        Some((ends, handle)) => Span::shaped(a, b, false, ends, handle, along0_m),
        None => Span::new(a, b, true, along0_m),
    };
    Span { water_z: true, owner, ..span }
}

/// Whether water lies underground on a span: at a point of it in the cell's grown square, every
/// `UNDERGROUND_STEP_M` along it, its level lies more than `depth_m` under the terrain. The grown square
/// holds the finest terrain of every cell that reads the point, so every cell decides alike where a cell
/// draws the span. Such a span is no part: the water ends at its vertices.
fn underground(input: &CellInput, a: &Vertex, b: &Vertex, span: &Span, depth_m: f64) -> bool {
    let [x0, y0, x1, y1] = input.key.grown();
    let n = (span.len_m / UNDERGROUND_STEP_M).ceil().max(1.0) as u32;
    (0..=n).any(|k| {
        let (p, z) = level_at(a, b, span, f64::from(k) / f64::from(n));
        let inside = (x0..=x1).contains(&p[0]) && (y0..=y1).contains(&p[1]);
        inside && input.terrain.height(GlobalPoint { x: p[0], y: p[1] }).is_some_and(|t| t - z > depth_m)
    })
}

/// Remove repeated vertices: the later one goes, its `side` joins the one that stays.
fn repeated(chain: &mut Chain, stats: &mut GraphStats) -> bool {
    let mut changed = false;
    let mut i = 1;
    while i < chain.vertices.len() {
        if chain.vertices[i].v.pos == chain.vertices[i - 1].v.pos {
            let side = chain.vertices[i].v.side;
            chain.vertices[i - 1].v.side |= side;
            chain.vertices.remove(i);
            // The zero-length span goes; the span after it starts at the vertex that stays.
            chain.spans.remove(i - 1);
            stats.repeated += 1;
            changed = true;
        } else {
            i += 1;
        }
    }
    changed
}

/// The parts of a cell and their rules, ordered by their vertex positions.
pub(crate) fn parts<'a>(
    input: &CellInput,
    sections: &'a Sections,
    stats: &mut GraphStats,
) -> (Vec<Part>, Vec<Rule<'a>>) {
    let classes: Vec<_> = input.features.iter().map(|f| classify(f, sections)).collect();
    let line: Vec<bool> = classes.iter().map(Option::is_some).collect();
    let water: Vec<bool> = classes.iter().map(|c| matches!(c, Some((Family::Water, _, Level::Ground, _)))).collect();
    let mut chains = chains(input, &line);
    for chain in &mut chains {
        repeated(chain, stats);
        if water[chain.feature] {
            if chain.before.is_empty() {
                chain.before = through(input, &water, chain, true).unwrap_or_default();
            }
            if chain.after.is_empty() {
                chain.after = through(input, &water, chain, false).unwrap_or_default();
            }
        }
        // A spike stays, and its two spans are chords: the test reads only the part.
        for i in 0..chain.vertices.len() {
            let (prev, next) = neighbours(chain, i);
            let spike = prev.zip(next).is_some_and(|(a, c)| turn_deg(a, chain.vertices[i].v.pos, c) > SPIKE_DEG);
            if spike {
                chain.vertices[i].corner = true;
                stats.spikes += 1;
            }
        }
        if let Some((_, _, _, Rule::Water(w))) = classes[chain.feature] {
            curve_water(chain, w.curve_turn_max_deg, w.curve_off_max_m);
        }
    }
    let chains: Vec<Chain> = chains.into_iter().flat_map(|c| trim(c, input.key.grown())).collect();
    let mut out: Vec<(Part, Rule<'a>)> = Vec::new();
    for chain in chains {
        let (family, class, level, rule) = classes[chain.feature].expect("a chain of a line feature");
        let keep_unknown = family == Family::Water && level == Level::Tunnel;
        let mut piece: Vec<Raw> = Vec::new();
        let mut spans: Vec<(f64, bool, [u32; 3], WaterShape)> = Vec::new();
        let mut flush = |piece: &mut Vec<Raw>,
                         spans: &mut Vec<(f64, bool, [u32; 3], WaterShape)>,
                         stats: &mut GraphStats| {
            if piece.len() >= 2 {
                // Water in a tunnel has no level: 0 marks it, it is no height.
                let vertices: Vec<Vertex> =
                    piece.iter().map(|r| Vertex { z_m: r.z.unwrap_or(0.0), ..r.v.clone() }).collect();
                let corner: Vec<bool> = piece.iter().map(|r| r.corner).collect();
                let spans = vertices
                    .windows(2)
                    .zip(spans.iter())
                    .enumerate()
                    .map(|(k, (pair, &(along0, chord, owner, shape)))| {
                        if let Rule::Water(_) = rule {
                            return water_span(&pair[0], &pair[1], along0, owner, shape);
                        }
                        let d = [(pair[1].pos[0] - pair[0].pos[0]) as f64, (pair[1].pos[1] - pair[0].pos[1]) as f64];
                        let chord = corner[k] || corner[k + 1] || chord;
                        let folded = !chord && folds(d, pair[0].tangent, pair[1].tangent);
                        stats.folded += u32::from(folded);
                        Span { owner, ..Span::new(&pair[0], &pair[1], chord || folded, along0) }
                    })
                    .collect();
                let part = Part {
                    feature: FeatureRef(chain.feature as u32),
                    family,
                    class,
                    level,
                    backward: chain.backward,
                    vertices,
                    spans,
                };
                out.push((part, rule));
            }
            piece.clear();
            spans.clear();
        };
        for (k, &span) in chain.spans.iter().enumerate() {
            let (a, b) = (&chain.vertices[k], &chain.vertices[k + 1]);
            let deep = match (rule, a.z.zip(b.z)) {
                (Rule::Water(w), Some((za, zb))) if level == Level::Ground => {
                    let (va, vb) = (Vertex { z_m: za, ..a.v.clone() }, Vertex { z_m: zb, ..b.v.clone() });
                    underground(input, &va, &vb, &water_span(&va, &vb, span.0, span.2, span.3), w.underground_m)
                }
                _ => false,
            };
            stats.underground += u32::from(deep);
            if keep_unknown || (a.z.is_some() && b.z.is_some() && !deep) {
                if piece.is_empty() {
                    piece.push(a.clone());
                }
                piece.push(b.clone());
                spans.push(span);
            } else {
                stats.no_height += 1;
                flush(&mut piece, &mut spans, stats);
            }
        }
        flush(&mut piece, &mut spans, stats);
    }
    out.sort_by(|(p, _), (q, _)| {
        let (a, b) = (p.vertices.iter().map(|v| v.pos), q.vertices.iter().map(|v| v.pos));
        a.cmp(b).then((p.family, p.class).cmp(&(q.family, q.class)))
    });
    out.into_iter().unzip()
}
