//! Stage 1: the parts of a cell. The owned runs of the read tiles join across tile lines, then each
//! part gets its family, class and level, its cleaning, and splits where its heights are unknown.

use std::collections::BTreeMap;

use super::{folds, water_tangents, Family, FeatureRef, GraphStats, Level, Part, Span, Vertex, SPIKE_DEG};
use crate::frame::GlobalPoint;
use crate::sections::{is_track, Sections, WaterSection, WaySection};
use crate::tile::{self, Feature, Geometry};
use crate::CellInput;

const WAYS: [&str; 6] = ["main-road", "road", "path", "railway", "tunnel", "gallery"];
const WATERS: [&str; 2] = ["river", "stream"];
/// The classes of the kind `path`.
const PATH_CLASSES: [&str; 7] = ["path", "footway", "cycleway", "bridleway", "steps", "pedestrian", "via_ferrata"];
/// Spans kept past each span that may meet the work area: the cleaning of a vertex reads its two spans.
const TRIM_SPANS: usize = 2;

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
            Rule::Way(s, _) => s.widths().outer_m(),
            Rule::Water(w) => w.half_m,
        }
    }

    /// A way tagged `ford`, or a `low_water_crossing`.
    pub fn ford(&self) -> bool {
        matches!(self, Rule::Way(_, true))
    }
}

/// The family, rank, level and rules of a line feature, or None for another kind.
fn classify<'a>(feature: &Feature, sections: &'a Sections) -> Option<(Family, u8, Level, Rule<'a>)> {
    let tag = |key: &str| feature.tags.get(key).map(String::as_str);
    let kind = feature.kind.as_str();
    if feature.geometry != Geometry::Line {
        return None;
    }
    if WATERS.contains(&kind) {
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

/// A joined part before cleaning: its vertices, per span the stroke metres at its start and
/// whether it has no alignment, and the tile's vertex before and after it, where the tile holds one.
struct Chain {
    feature: usize,
    backward: bool,
    vertices: Vec<Raw>,
    spans: Vec<(f64, bool, [u32; 3])>,
    before: Option<[i64; 2]>,
    after: Option<[i64; 2]>,
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
        let before = r0.from.checked_sub(1).map(|i| key(part(&r0).points[i]));
        let mut chain = Chain {
            feature: r0.feature,
            backward: false,
            vertices: Vec::new(),
            spans: Vec::new(),
            before,
            after: None,
        };
        let mut at = Some(first);
        while let Some(i) = at.filter(|&i| !seen[i]) {
            seen[i] = true;
            let r = runs[i];
            let p = part(&r);
            chain.backward = p.plan.as_ref().is_some_and(|plan| plan.backward);
            let vertices = tile_vertices(p);
            let along = tile_along(p, &vertices);
            let skip = usize::from(!chain.vertices.is_empty());
            chain.vertices.extend(vertices[r.from + skip..=r.to].iter().cloned());
            let owner = [p.tile.0, p.tile.1, input.features[r.feature].index];
            chain.spans.extend((r.from..r.to).map(|k| (along[k], p.plan.is_none(), owner)));
            chain.after = p.points.get(r.to + 1).map(|&q| key(q));
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
        out.push(Chain {
            feature: chain.feature,
            backward: chain.backward,
            vertices: chain.vertices[start..=k].to_vec(),
            spans: chain.spans[start..k].to_vec(),
            before: if start > 0 { Some(chain.vertices[start - 1].v.pos) } else { chain.before },
            after: if k < n { Some(chain.vertices[k + 1].v.pos) } else { chain.after },
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
    let prev = if i > 0 { Some(chain.vertices[i - 1].v.pos) } else { chain.before };
    let next = if i + 1 < n { Some(chain.vertices[i + 1].v.pos) } else { chain.after };
    (prev.filter(|&q| q != here), next.filter(|&q| q != here))
}

/// The tangents and corners of water (`water_tangents`). A chain end that is no end of the way reads
/// the tile's vertex past it, so every cell that samples a span gives it one curve.
fn curve_water(chain: &mut Chain, turn_max_deg: f64) {
    let points: Vec<[f64; 2]> = chain
        .before
        .into_iter()
        .chain(chain.vertices.iter().map(|r| r.v.pos))
        .chain(chain.after)
        .map(|p| p.map(|c| c as f64))
        .collect();
    let tangents = water_tangents(&points, turn_max_deg).into_iter().skip(usize::from(chain.before.is_some()));
    for (r, (tangent, corner)) in chain.vertices.iter_mut().zip(tangents) {
        r.v.tangent = tangent;
        r.corner |= corner;
    }
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
    let mut chains = chains(input, &line);
    for chain in &mut chains {
        repeated(chain, stats);
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
            curve_water(chain, w.curve_turn_max_deg);
        }
    }
    let chains: Vec<Chain> = chains.into_iter().flat_map(|c| trim(c, input.key.grown())).collect();
    let mut out: Vec<(Part, Rule<'a>)> = Vec::new();
    for chain in chains {
        let (family, class, level, rule) = classes[chain.feature].expect("a chain of a line feature");
        let keep_unknown = family == Family::Water && level == Level::Tunnel;
        let mut piece: Vec<Raw> = Vec::new();
        let mut spans: Vec<(f64, bool, [u32; 3])> = Vec::new();
        let mut flush = |piece: &mut Vec<Raw>, spans: &mut Vec<(f64, bool, [u32; 3])>, stats: &mut GraphStats| {
            if piece.len() >= 2 {
                // Water in a tunnel has no level: 0 marks it, it is no height.
                let vertices: Vec<Vertex> =
                    piece.iter().map(|r| Vertex { z_m: r.z.unwrap_or(0.0), ..r.v.clone() }).collect();
                let water = family == Family::Water;
                let corner: Vec<bool> = piece.iter().map(|r| r.corner).collect();
                let spans = vertices
                    .windows(2)
                    .zip(spans.iter())
                    .enumerate()
                    .map(|(k, (pair, &(along0, chord, owner)))| {
                        let off_max = match rule {
                            Rule::Water(w) => w.curve_off_max_m,
                            Rule::Way(..) => f64::INFINITY,
                        };
                        let chord = corner[k]
                            || corner[k + 1]
                            || (chord && !water)
                            || (water && super::curve::off_chord_m(&pair[0], &pair[1]) > off_max);
                        let d = [(pair[1].pos[0] - pair[0].pos[0]) as f64, (pair[1].pos[1] - pair[0].pos[1]) as f64];
                        let folded = !chord && folds(d, pair[0].tangent, pair[1].tangent);
                        stats.folded += u32::from(folded);
                        Span { linear_z: water, owner, ..Span::new(&pair[0], &pair[1], chord || folded, along0) }
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
            if keep_unknown || (a.z.is_some() && b.z.is_some()) {
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
