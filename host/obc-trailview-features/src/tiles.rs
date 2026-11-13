//! The tiles of the archive: the features cut to the buffered square of each tile, in tile units,
//! and their encoding (`features_format` of `specs/trailview-features.md`).

use std::collections::{BTreeSet, HashMap};
use std::f64::consts::TAU;

use obc_ground_plan::sections::WATER_KINDS;

use crate::geometry::bounds;
use crate::{world, Part, Plan, Tags, EQUATOR_M};

pub const FORMAT: u32 = 4;
/// A unit of a zoom-13 tile is about 0.2 m in the Alps, a texel of the finest terrain node.
pub const EXTENT: u32 = 16384;
pub const MIN_ZOOM: u8 = 8;
pub const MAX_ZOOM: u8 = 13;
/// The widest stamp, a road cone, reaches 25 m past the road edge: 40 m holds it for a road up to
/// 30 m wide.
pub const BUFFER_M: f64 = 40.0;
/// Below MAX_ZOOM a tile drops detail under a quarter texel of a 512-texel terrain node.
pub const TOLERANCE: f64 = EXTENT as f64 / 2048.0;

const EXTENT_F: f64 = EXTENT as f64;
/// A feature with an alignment counts its coordinates in quarters of a tile unit: a tile unit of 0.2 m
/// would give a smooth curve a wobble of 10 cm.
pub const FINE: i64 = 4;
/// A turn of a tangent in codes, 0.35°: 1.4 cm off the true curve over a chord of 30 m.
const TURN: i64 = 1024;
/// The codes of a rise per metre: 0.4 %, or 1 cm of height over a chord of 30 m.
const GRADE: f64 = 250.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Geometry {
    Point,
    Line,
    Area,
}

impl Geometry {
    fn code(self) -> u64 {
        match self {
            Geometry::Point => 0,
            Geometry::Line => 1,
            Geometry::Area => 2,
        }
    }
}

/// The alignment of a part in the codes of the format.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Aligned {
    /// Decimetres along the stroke at the first vertex, twice, and one more when they fall along the part.
    pub along: u64,
    /// Per vertex: 1/TURN of a turn.
    pub tangent: Vec<i64>,
    /// Per vertex: 1/GRADE.
    pub grade: Vec<i64>,
    /// Per vertex: 1/1000.
    pub bank: Vec<i64>,
    /// Per vertex: centimetres.
    pub widen: Vec<u64>,
    /// Per vertex: the bits `SIDE_BACK` and `SIDE_ON`.
    pub side: Vec<u64>,
    /// Per vertex: the bits `WALK_LEFT` and `WALK_RIGHT`.
    pub walk: Vec<u64>,
}

/// A part in integer tile units, or in quarters of them in a feature with an alignment, with a height
/// per point (NaN where unknown) or none.
#[derive(Clone, Debug, PartialEq)]
pub struct Quantized {
    pub outer: bool,
    pub xy: Vec<[i64; 2]>,
    pub z: Option<Vec<f64>>,
    pub plan: Option<Aligned>,
}

/// A feature of a tile, as `decode` gives it and `encode` takes it.
#[cfg(test)]
#[derive(Clone, Debug, PartialEq)]
pub struct Feature {
    pub kind: String,
    pub tags: Tags,
    pub geometry: Geometry,
    pub parts: Vec<Quantized>,
}

/// The streams of a tile, in their order in the tile. Each field of its own stream lets gzip find
/// the runs of that field.
#[derive(Default)]
struct Streams {
    heads: Vec<u8>,
    tags: Vec<u8>,
    parts: Vec<u8>,
    vertices: Vec<u8>,
    along: Vec<u8>,
    tangent: Vec<u8>,
    grade: Vec<u8>,
    bank: Vec<u8>,
    words: Vec<u8>,
}

/// The encoder of one tile.
#[derive(Default)]
pub struct Tile {
    strings: HashMap<String, u64>,
    order: Vec<String>,
    streams: Streams,
    count: u64,
    /// The features that can take more tags after the cut: their position, their id and where their tags start in
    /// the tags stream.
    ids: Vec<(u32, u32, usize)>,
}

fn read_varint(bytes: &[u8], mut at: usize) -> (u64, usize) {
    let (mut value, mut shift) = (0u64, 0);
    loop {
        let byte = bytes[at];
        at += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte < 0x80 {
            return (value, at);
        }
        shift += 7;
    }
}

fn varint(out: &mut Vec<u8>, mut value: u64) {
    while value > 0x7f {
        out.push(value as u8 & 0x7f | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn zigzag(value: i64) -> u64 {
    ((value << 1) ^ (value >> 63)) as u64
}

impl Tile {
    fn string(&mut self, text: &str) -> u64 {
        if let Some(&at) = self.strings.get(text) {
            return at;
        }
        let at = self.order.len() as u64;
        self.strings.insert(text.to_string(), at);
        self.order.push(text.to_string());
        at
    }

    /// Add a feature, with `id` where it can take more tags after the cut (`bytes_with`). A height is in metres; a
    /// part without heights in a feature with some has an unknown height at each point.
    pub fn add(&mut self, kind: &str, tags: &Tags, geometry: Geometry, parts: &[Quantized], id: Option<u32>) {
        if let Some(id) = id {
            self.ids.push((self.count as u32, id, self.streams.tags.len()));
        }
        let kind = self.string(kind);
        let tags: Vec<(u64, u64)> = tags.iter().map(|(key, value)| (self.string(key), self.string(value))).collect();
        let out = &mut self.streams;
        let solved = parts.iter().any(|part| part.z.is_some());
        let planned = parts.iter().any(|part| part.plan.is_some());
        let flags = u64::from(planned) << 3 | u64::from(solved) << 2 | geometry.code();
        varint(&mut out.heads, kind);
        varint(&mut out.heads, (parts.len() as u64) << 4 | flags);
        varint(&mut out.tags, tags.len() as u64);
        for (key, value) in tags {
            varint(&mut out.tags, key);
            varint(&mut out.tags, value);
        }
        let (mut x, mut y, mut height) = (0, 0, 0i64);
        for part in parts {
            varint(&mut out.parts, (part.xy.len() as u64) << 1 | u64::from(part.outer));
            for (index, &[px, py]) in part.xy.iter().enumerate() {
                varint(&mut out.vertices, zigzag(px - x));
                varint(&mut out.vertices, zigzag(py - y));
                (x, y) = (px, py);
                if !solved {
                    continue;
                }
                match part.z.as_ref().map(|z| z[index]).filter(|value| !value.is_nan()) {
                    None => out.vertices.push(0),
                    Some(value) => {
                        let value = (value * 100.0).round_ties_even() as i64;
                        varint(&mut out.vertices, zigzag(value - height) + 1);
                        height = value;
                    }
                }
            }
            if planned {
                out.plan(part.plan.as_ref());
            }
        }
        self.count += 1;
    }

    /// The bytes of the tile.
    pub fn bytes(&self) -> Vec<u8> {
        self.bytes_with(&|_| None)
    }

    pub fn has_ids(&self) -> bool {
        !self.ids.is_empty()
    }

    /// The id of the feature at a position of the tile, where it has one.
    pub fn id(&self, position: u32) -> Option<u32> {
        let k = self.ids.binary_search_by_key(&position, |x| x.0).ok()?;
        Some(self.ids[k].1)
    }

    /// The bytes of the tile with the tags of `more` on the features of each id: its own keys, which the feature
    /// lacks. The tags of a feature stay sorted by key.
    pub fn bytes_with<'a>(&self, more: &dyn Fn(u32) -> Option<&'a Tags>) -> Vec<u8> {
        let (mut strings, mut order) = (self.strings.clone(), self.order.clone());
        let mut string = |order: &mut Vec<String>, text: &str| {
            *strings.entry(text.to_string()).or_insert_with(|| {
                order.push(text.to_string());
                order.len() as u64 - 1
            })
        };
        let (mut tags, mut at) = (Vec::new(), 0);
        for &(_, id, start) in &self.ids {
            let Some(more) = more(id).filter(|m| !m.is_empty()) else { continue };
            tags.extend(&self.streams.tags[at..start]);
            let (count, mut i) = read_varint(&self.streams.tags, start);
            let mut pairs = Vec::new();
            for _ in 0..count {
                let (key, next) = read_varint(&self.streams.tags, i);
                let (value, next) = read_varint(&self.streams.tags, next);
                pairs.push((key, value));
                i = next;
            }
            at = i;
            for (key, value) in more {
                pairs.push((string(&mut order, key), string(&mut order, value)));
            }
            pairs.sort_by(|a, b| order[a.0 as usize].cmp(&order[b.0 as usize]));
            varint(&mut tags, pairs.len() as u64);
            for (key, value) in pairs {
                varint(&mut tags, key);
                varint(&mut tags, value);
            }
        }
        tags.extend(&self.streams.tags[at..]);
        let mut out = Vec::new();
        varint(&mut out, order.len() as u64);
        for text in &order {
            varint(&mut out, text.len() as u64);
            out.extend(text.as_bytes());
        }
        varint(&mut out, self.count);
        let Streams { heads, parts, vertices, along, tangent, grade, bank, words, .. } = &self.streams;
        for stream in [heads, &tags, parts, vertices, along, tangent, grade, bank, words] {
            out.extend(stream);
        }
        out
    }
}

impl Streams {
    /// The alignment of a part. A tangent and a grade are the difference from the vertex before.
    fn plan(&mut self, plan: Option<&Aligned>) {
        let Some(plan) = plan else { return self.along.push(0) };
        varint(&mut self.along, plan.along + 1);
        let mut last = 0;
        for &tangent in &plan.tangent {
            varint(&mut self.tangent, zigzag((tangent - last + TURN / 2).rem_euclid(TURN) - TURN / 2));
            last = tangent;
        }
        last = 0;
        for &grade in &plan.grade {
            varint(&mut self.grade, zigzag(grade - last));
            last = grade;
        }
        plan.bank.iter().for_each(|&bank| varint(&mut self.bank, zigzag(bank)));
        // Few vertices have a widening, a sidewalk or a side, so the three share a word.
        for ((&widen, &walk), &side) in plan.widen.iter().zip(&plan.walk).zip(&plan.side) {
            varint(&mut self.words, widen * 16 + walk * 4 + side);
        }
    }
}

/// The bytes of a tile that holds `features`.
#[cfg(test)]
pub fn encode(features: &[Feature]) -> Vec<u8> {
    let mut tile = Tile::default();
    for feature in features {
        tile.add(&feature.kind, &feature.tags, feature.geometry, &feature.parts, None);
    }
    tile.bytes()
}

/// The features of a tile, through the reader of `obc-ground-plan`.
#[cfg(test)]
pub fn decode(data: &[u8]) -> Result<Vec<Feature>, String> {
    use obc_ground_plan::tile::{self, Geometry as Read};
    let features = tile::decode(data).map_err(|e| e.to_string())?;
    let feature = |f: tile::RawFeature| Feature {
        kind: f.kind,
        tags: f.tags,
        geometry: match f.geometry {
            Read::Point => Geometry::Point,
            Read::Line => Geometry::Line,
            Read::Area => Geometry::Area,
        },
        parts: f
            .parts
            .into_iter()
            .map(|p| Quantized {
                outer: p.outer,
                xy: p.xy,
                z: p.z.map(|z| z.into_iter().map(|cm| cm.map_or(f64::NAN, |cm| cm as f64 / 100.0)).collect()),
                plan: p.plan.map(|plan| Aligned {
                    along: plan.along,
                    tangent: plan.tangent,
                    grade: plan.grade,
                    bank: plan.bank,
                    widen: plan.widen,
                    side: plan.side.into_iter().map(u64::from).collect(),
                    walk: plan.walk.into_iter().map(u64::from).collect(),
                }),
            })
            .collect(),
    };
    Ok(features.into_iter().map(feature).collect())
}

/// The box (x0, y0, x1, y1) of the points of the parts, or None when they have none.
fn extent(parts: &[Part]) -> Option<[f64; 4]> {
    Some(bounds(parts.iter().flat_map(|part| &part.points))).filter(|b| b[0] <= b[2])
}

/// What a tile keeps of a feature besides its parts.
#[derive(Clone, Copy)]
struct Named<'a> {
    kind: &'a str,
    geometry: Geometry,
    tags: &'a Tags,
    id: Option<u32>,
}

/// The tiles of a region from MIN_ZOOM to MAX_ZOOM that hold at least one feature.
pub struct Tiles {
    region: [f64; 4],
    pub tiles: HashMap<(u8, u32, u32), Tile>,
    pads: HashMap<(u8, i64), f64>,
}

impl Tiles {
    /// The tiles of the region `bounds` (west, south, east, north in degrees).
    pub fn new(bounds: [f64; 4]) -> Self {
        let [west, south, east, north] = bounds;
        let ([x0, y0], [x1, y1]) = (world(west, north), world(east, south));
        Tiles { region: [x0, y0, x1, y1], tiles: HashMap::new(), pads: HashMap::new() }
    }

    /// The columns and rows (c0, r0, c1, r1) of the tiles of the region at `zoom`.
    pub fn span(&self, zoom: u8) -> [i64; 4] {
        let n = (1u64 << zoom) as f64;
        let [x0, y0, x1, y1] = self.region.map(|value| value * n);
        [x0.floor() as i64, y0.floor() as i64, x1.ceil() as i64 - 1, y1.ceil() as i64 - 1]
    }

    /// Whether the parts come within one tile of the tiles of the region at `zoom`.
    pub fn meets(&self, zoom: u8, parts: &[Part]) -> bool {
        let n = (1u64 << zoom) as f64;
        let [c0, r0, c1, r1] = self.span(zoom).map(|value| value as f64);
        let Some(box_) = extent(parts) else { return false };
        let [x0, y0, x1, y1] = box_.map(|value| value * n);
        x0 < c1 + 2.0 && x1 > c0 - 1.0 && y0 < r1 + 2.0 && y1 > r0 - 1.0
    }

    /// The buffer of the tiles of `row` in tile units: BUFFER_M on the ground at the edge nearer the
    /// pole, so at least BUFFER_M everywhere.
    fn pad(&mut self, zoom: u8, row: i64) -> f64 {
        *self.pads.entry((zoom, row)).or_insert_with(|| {
            let n = 1i64 << zoom;
            let row = row.clamp(0, n - 1);
            let latitude =
                |edge: i64| (std::f64::consts::PI * (1.0 - 2.0 * edge as f64 / n as f64)).sinh().atan().abs();
            let latitude = latitude(row).max(latitude(row + 1));
            BUFFER_M * n as f64 * EXTENT_F / (EQUATOR_M * latitude.cos())
        })
    }

    /// Add the feature to each tile of `zoom` in the region whose buffered square it meets, cut to
    /// that square, with its `id` in each (`Tile::add`). Whether a tile took it.
    pub fn add(
        &mut self,
        zoom: u8,
        kind: &str,
        geometry: Geometry,
        tags: &Tags,
        parts: &[Part],
        id: Option<u32>,
    ) -> bool {
        let scale = (1u64 << zoom) as f64 * EXTENT_F;
        let parts: Vec<Part> = parts
            .iter()
            .map(|part| Part {
                outer: part.outer,
                points: part.points.iter().map(|&[x, y]| [x * scale, y * scale]).collect(),
                z: part.z.clone(),
                plan: part.plan.clone(),
            })
            .collect();
        let Some(box_) = extent(&parts) else { return false };
        let row = |value: f64| (value / EXTENT_F).floor() as i64;
        // The buffer grows towards the pole: twice that of the outer rows bounds every row's.
        let margin = 2.0 * self.pad(zoom, row(box_[1]) - 1).max(self.pad(zoom, row(box_[3]) + 1));
        let [c0, r0, c1, r1] = self.span(zoom);
        let (c0, r0) = (c0.max(row(box_[0] - margin)), r0.max(row(box_[1] - margin)));
        let (c1, r1) = (c1.min(row(box_[2] + margin)), r1.min(row(box_[3] + margin)));
        let feature = Named { kind, geometry, tags, id };
        c0 <= c1 && r0 <= r1 && self.cut(zoom, &feature, parts, box_, [c0, r0, c1, r1])
    }

    /// Cut the parts to the buffered square of the tiles c0..c1 × r0..r1, then halve the range: a
    /// feature over many tiles is not cut once for each of them. Whether a tile took it.
    fn cut(
        &mut self,
        zoom: u8,
        feature: &Named,
        mut parts: Vec<Part>,
        mut box_: [f64; 4],
        [c0, r0, c1, r1]: [i64; 4],
    ) -> bool {
        let Named { kind, geometry, tags, id } = *feature;
        let buffer = self.pad(zoom, r0).max(self.pad(zoom, r1));
        let extent_i = i64::from(EXTENT);
        let rect = [
            (c0 * extent_i) as f64 - buffer,
            (r0 * extent_i) as f64 - buffer,
            ((c1 + 1) * extent_i) as f64 + buffer,
            ((r1 + 1) * extent_i) as f64 + buffer,
        ];
        if box_[0] > rect[2] || box_[2] < rect[0] || box_[1] > rect[3] || box_[3] < rect[1] {
            return false;
        }
        if !(rect[0] <= box_[0] && box_[2] <= rect[2] && rect[1] <= box_[1] && box_[3] <= rect[3]) {
            parts = clipped(geometry, &parts, rect, reach(kind));
            match extent(&parts) {
                Some(found) => box_ = found,
                None => return false,
            }
        }
        if (c0, r0) == (c1, r1) {
            let cut = quantized(geometry, &parts, c0 * extent_i, r0 * extent_i);
            if !cut.is_empty() {
                self.tiles.entry((zoom, c0 as u32, r0 as u32)).or_default().add(kind, tags, geometry, &cut, id);
            }
            return !cut.is_empty();
        }
        let halves = if c1 - c0 >= r1 - r0 {
            let middle = (c0 + c1).div_euclid(2);
            [[c0, r0, middle, r1], [middle + 1, r0, c1, r1]]
        } else {
            let middle = (r0 + r1).div_euclid(2);
            [[c0, r0, c1, middle], [c0, middle + 1, c1, r1]]
        };
        let first = self.cut(zoom, feature, parts.clone(), box_, halves[0]);
        let second = self.cut(zoom, feature, parts, box_, halves[1]);
        first || second
    }
}

/// Whether the segment a-b meets `rect` (x0, y0, x1, y1), by Liang-Barsky.
fn meets([ax, ay]: [f64; 2], [bx, by]: [f64; 2], [x0, y0, x1, y1]: [f64; 4]) -> bool {
    let (dx, dy) = (bx - ax, by - ay);
    let (mut enter, mut leave) = (0.0f64, 1.0f64);
    for (p, q) in [(-dx, ax - x0), (dx, x1 - ax), (-dy, ay - y0), (dy, y1 - ay)] {
        if p == 0.0 {
            if q < 0.0 {
                return false;
            }
        } else if p < 0.0 {
            enter = enter.max(q / p);
        } else {
            leave = leave.min(q / p);
        }
    }
    enter <= leave
}

/// The pieces of a polyline that meet `rect`, as lists of vertex indices. A piece keeps the segments
/// that meet `rect` whole, and `reach` more vertices at each end: the tiles that hold a segment hold the
/// same segment and the same vertices about its ends.
fn clip_line(points: &[[f64; 2]], rect: [f64; 4], reach: usize) -> Vec<Vec<usize>> {
    let mut keep = vec![false; points.len()];
    for k in 0..points.len().saturating_sub(1) {
        if meets(points[k], points[k + 1], rect) {
            keep[k.saturating_sub(reach)..(k + 2 + reach).min(points.len())].fill(true);
        }
    }
    let mut pieces: Vec<Vec<usize>> = Vec::new();
    let mut piece = Vec::new();
    for (k, &kept) in keep.iter().enumerate() {
        if kept {
            piece.push(k);
        } else if !piece.is_empty() {
            pieces.push(std::mem::take(&mut piece));
        }
    }
    if !piece.is_empty() {
        pieces.push(piece);
    }
    pieces
}

/// The vertex indices of a ring cut to `rect`, one side at a time. An edge that crosses a side
/// stays whole, and the vertices outside it join directly: such a join lies outside the side, so
/// inside `rect` the ring and its edges are the original ones.
fn clip_ring(ring: &[[f64; 2]], rect: [f64; 4]) -> Vec<usize> {
    let mut indices: Vec<usize> = (0..ring.len()).collect();
    for (axis, bound, side) in [(0, rect[0], 1.0), (0, rect[2], -1.0), (1, rect[1], 1.0), (1, rect[3], -1.0)] {
        let inside = |index: usize| (ring[index][axis] - bound) * side >= 0.0;
        let mut result = Vec::new();
        for (n, &index) in indices.iter().enumerate() {
            let previous = indices[(n + indices.len() - 1) % indices.len()];
            if inside(index) {
                if !inside(previous) {
                    result.push(previous);
                }
                result.push(index);
            } else if inside(previous) {
                result.push(index);
            }
        }
        let count = result.len();
        indices = (0..count)
            .filter(|&n| count == 1 || result[n] != result[(n + count - 1) % count])
            .map(|n| result[n])
            .collect();
    }
    indices
}

/// The vertices that a line of `kind` keeps past the segments that meet a square: the ground plan draws
/// the curve of line water from the tile's vertices, and a span's curve reads two vertices on each side
/// of it (`obc_ground_plan::graph::water_spans`); every other line carries its plan.
fn reach(kind: &str) -> usize {
    if WATER_KINDS.contains(&kind) {
        2
    } else {
        1
    }
}

/// Parts cut to `rect`, a line with `reach` vertices past the segments that meet it. The cut makes no
/// new vertex.
fn clipped(geometry: Geometry, parts: &[Part], rect: [f64; 4], reach: usize) -> Vec<Part> {
    let pick = |outer: bool, part: &Part, indices: &[usize]| Part {
        outer,
        points: indices.iter().map(|&i| part.points[i]).collect(),
        z: part.z.as_ref().map(|z| indices.iter().map(|&i| z[i]).collect()),
        plan: part.plan.as_ref().map(|plan| indices.iter().map(|&i| plan[i]).collect()),
    };
    let [x0, y0, x1, y1] = rect;
    match geometry {
        Geometry::Point => parts
            .iter()
            .map(|part| {
                let inside: Vec<usize> = (0..part.points.len())
                    .filter(|&i| {
                        let [x, y] = part.points[i];
                        x0 <= x && x <= x1 && y0 <= y && y <= y1
                    })
                    .collect();
                pick(part.outer, part, &inside)
            })
            .collect(),
        Geometry::Line => parts
            .iter()
            .flat_map(|part| clip_line(&part.points, rect, reach).into_iter().map(move |piece| (part, piece)))
            .map(|(part, piece)| pick(false, part, &piece))
            .collect(),
        Geometry::Area => parts.iter().map(|part| pick(part.outer, part, &clip_ring(&part.points, rect))).collect(),
    }
}

fn twice_area(ring: &[[i64; 2]]) -> i128 {
    (0..ring.len())
        .map(|i| {
            let ([ax, ay], [bx, by]) = (ring[i], ring[(i + 1) % ring.len()]);
            i128::from(ax) * i128::from(by) - i128::from(bx) * i128::from(ay)
        })
        .sum()
}

/// A tangent in radians as a tile rounds it.
pub fn rounded_tangent(tangent: f64) -> f64 {
    (tangent * TURN as f64 / TAU).round_ties_even() * TAU / TURN as f64
}

/// A grade as a tile rounds it.
pub fn rounded_grade(grade: f64) -> f64 {
    (grade * GRADE).round_ties_even() / GRADE
}

/// The codes of the alignment of the vertices `indices` of a part.
fn aligned(plan: &[Plan], indices: &[usize]) -> Aligned {
    let (first, last) = (plan[indices[0]].along, plan[indices[indices.len() - 1]].along);
    let code = |value: f64, scale: f64| (value * scale).round_ties_even() as i64;
    let column = |value: fn(&Plan) -> f64, scale: f64| indices.iter().map(|&i| code(value(&plan[i]), scale)).collect();
    let turn = TURN as f64 / TAU;
    Aligned {
        along: code(first, 10.0) as u64 * 2 + u64::from(last < first),
        tangent: indices.iter().map(|&i| code(plan[i].tangent, turn).rem_euclid(TURN)).collect(),
        grade: column(|p| p.grade, GRADE),
        bank: column(|p| p.bank, 1000.0),
        widen: indices.iter().map(|&i| code(plan[i].widen, 100.0) as u64).collect(),
        side: indices.iter().map(|&i| u64::from(plan[i].side)).collect(),
        walk: indices.iter().map(|&i| u64::from(plan[i].walk)).collect(),
    }
}

/// Parts in integer tile units, or in quarters of them in a feature with an alignment. A part without
/// enough distinct points goes, and the holes of an outer ring that goes go with it.
fn quantized(geometry: Geometry, parts: &[Part], left: i64, top: i64) -> Vec<Quantized> {
    let unit = if parts.iter().any(|part| part.plan.is_some()) { FINE } else { 1 };
    let (mut result, mut kept) = (Vec::new(), false);
    for part in parts {
        // A part with no heights, or an empty list of them, has none.
        let z = part.z.as_ref().filter(|z| !z.is_empty());
        let (mut xy, mut heights): (Vec<[i64; 2]>, Vec<f64>) = (Vec::new(), Vec::new());
        let mut indices = Vec::new();
        for (n, &[x, y]) in part.points.iter().enumerate() {
            let whole = part.plan.as_ref().is_some_and(|plan| plan[n].whole);
            let round = |value: f64| {
                let fine = if whole { value.round_ties_even() * unit as f64 } else { value * unit as f64 };
                fine.round_ties_even() as i64
            };
            let point = [round(x) - left * unit, round(y) - top * unit];
            if xy.last() != Some(&point) {
                xy.push(point);
                heights.push(z.map_or(f64::NAN, |z| z[n]));
                indices.push(n);
            }
        }
        match geometry {
            Geometry::Area => {
                if xy.len() > 1 && xy[0] == xy[xy.len() - 1] {
                    xy.pop();
                    heights.pop();
                }
                let solid = xy.len() >= 3 && twice_area(&xy) != 0;
                if part.outer {
                    kept = solid;
                }
                if !(kept && solid) {
                    continue;
                }
            }
            Geometry::Line if xy.len() < 2 => continue,
            _ if xy.is_empty() => continue,
            _ => {}
        }
        let plan = part.plan.as_ref().map(|plan| aligned(plan, &indices));
        result.push(Quantized { outer: part.outer, xy, z: z.map(|_| heights), plan });
    }
    result
}

/// Douglas-Peucker on each part, with no alignment: its vertices stay on the curve, and a chord of a low
/// zoom is drawn straight. The `anchors` of each part, the indices of vertices that other
/// features share, and the ends of a line stay: junctions and the joins of rivers and lakes keep
/// their places. The rings of an area then take back vertices until no two of their edges cross.
pub fn simplified(geometry: Geometry, parts: &[Part], anchors: &[Vec<usize>], tolerance: f64) -> Vec<Part> {
    if geometry == Geometry::Point {
        return parts.to_vec();
    }
    let area = geometry == Geometry::Area;
    // Per part: where its sequence starts in its points, the sequence, and the indices kept of it.
    let mut sequences: Vec<(Vec<[f64; 2]>, Vec<usize>)> = Vec::new();
    for (part, fixed) in parts.iter().zip(anchors) {
        let points = &part.points;
        let (sequence, stops): (Vec<[f64; 2]>, BTreeSet<usize>) = if area {
            // A ring turns to start at its first anchor and repeats it at its end.
            let start = fixed.first().copied().unwrap_or(0);
            let count = points.len();
            let mut stops: BTreeSet<usize> =
                fixed.iter().map(|&index| (index as i64 - start as i64).rem_euclid(count as i64) as usize).collect();
            stops.extend([0, count]);
            let sequence = points[start..].iter().chain(&points[..start + 1]).copied().collect();
            (sequence, stops)
        } else {
            let mut stops: BTreeSet<usize> = fixed.iter().copied().collect();
            stops.extend([0, points.len() - 1]);
            (points.clone(), stops)
        };
        let kept = reduced(&sequence, &stops.into_iter().collect::<Vec<_>>(), tolerance);
        sequences.push((sequence, kept));
    }
    if area {
        untangle(&mut sequences);
    }
    parts
        .iter()
        .zip(&sequences)
        .zip(anchors)
        .map(|((part, (_, kept)), fixed)| {
            let start = if area { fixed.first().copied().unwrap_or(0) } else { 0 };
            let kept = if area { &kept[..kept.len() - 1] } else { &kept[..] };
            let indices: Vec<usize> = kept.iter().map(|&index| (start + index) % part.points.len()).collect();
            Part {
                outer: part.outer,
                points: indices.iter().map(|&i| part.points[i]).collect(),
                z: part.z.as_ref().map(|z| indices.iter().map(|&i| z[i]).collect()),
                plan: None,
            }
        })
        .collect()
}

/// The index between i and j of the point farthest from the chord i-j, and its distance.
fn farthest(points: &[[f64; 2]], i: usize, j: usize) -> (Option<usize>, f64) {
    let ([ax, ay], [bx, by]) = (points[i], points[j]);
    let (dx, dy) = (bx - ax, by - ay);
    let length = dx.hypot(dy);
    let (mut distance, mut index) = (-1.0, None);
    for (k, &[px, py]) in points.iter().enumerate().take(j).skip(i + 1) {
        let d = if length != 0.0 { (dx * (ay - py) - dy * (ax - px)).abs() / length } else { (px - ax).hypot(py - ay) };
        if d > distance {
            (distance, index) = (d, Some(k));
        }
    }
    (index, distance)
}

/// The sorted indices that Douglas-Peucker keeps between the stops.
fn reduced(points: &[[f64; 2]], stops: &[usize], tolerance: f64) -> Vec<usize> {
    let mut keep: BTreeSet<usize> = stops.iter().copied().collect();
    let mut spans: Vec<(usize, usize)> = stops.windows(2).map(|pair| (pair[0], pair[1])).collect();
    while let Some((i, j)) = spans.pop() {
        let (index, distance) = farthest(points, i, j);
        if let Some(index) = index.filter(|_| distance > tolerance) {
            keep.insert(index);
            spans.extend([(i, index), (index, j)]);
        }
    }
    keep.into_iter().collect()
}

/// Give each edge that crosses another edge of the rings the vertex at which Douglas-Peucker
/// would split it next, until no edges cross. Rings without crossings at full detail end without
/// crossings.
fn untangle(sequences: &mut [(Vec<[f64; 2]>, Vec<usize>)]) {
    loop {
        let mut added = BTreeSet::new();
        for (ring, edge) in crossings(sequences) {
            let (points, kept) = &sequences[ring];
            if let (Some(index), _) = farthest(points, kept[edge], kept[edge + 1]) {
                added.insert((ring, index));
            }
        }
        if added.is_empty() {
            return;
        }
        for (ring, index) in added {
            sequences[ring].1.push(index);
        }
        for (_, kept) in sequences.iter_mut() {
            kept.sort_unstable();
        }
    }
}

fn crosses(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> bool {
    let side = |p: [f64; 2], q: [f64; 2], r: [f64; 2]| (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0]);
    side(a, b, c) * side(a, b, d) < 0.0 && side(c, d, a) * side(c, d, b) < 0.0
}

/// (ring, edge) of each kept edge that crosses another one. A grid of about one edge per cell
/// finds the pairs to test.
fn crossings(sequences: &[(Vec<[f64; 2]>, Vec<usize>)]) -> BTreeSet<(usize, usize)> {
    let mut edges = Vec::new();
    for (ring, (points, kept)) in sequences.iter().enumerate() {
        for n in 0..kept.len().saturating_sub(1) {
            edges.push((ring, n, points[kept[n]], points[kept[n + 1]]));
        }
    }
    let mut found = BTreeSet::new();
    if edges.len() < 4 {
        return found;
    }
    let ends = || edges.iter().flat_map(|e| [e.2, e.3]);
    let fold = |pick: fn([f64; 2]) -> f64| {
        ends().map(pick).fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v), hi.max(v)))
    };
    let ((x0, x1), (y0, y1)) = (fold(|p| p[0]), fold(|p| p[1]));
    let cell = (x1 - x0).max(y1 - y0) / (edges.len() as u64).isqrt() as f64;
    let cell = if cell == 0.0 { 1.0 } else { cell };
    let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (number, &(_, _, a, b)) in edges.iter().enumerate() {
        let index = |v: f64| (v / cell).floor() as i64;
        for column in index(a[0].min(b[0]))..=index(a[0].max(b[0])) {
            for row in index(a[1].min(b[1]))..=index(a[1].max(b[1])) {
                grid.entry((column, row)).or_default().push(number);
            }
        }
    }
    for members in grid.values() {
        for (position, &first) in members.iter().enumerate() {
            for &second in &members[position + 1..] {
                let ((r1, n1, a, b), (r2, n2, c, d)) = (edges[first], edges[second]);
                let distinct = a != b && a != c && a != d && b != c && b != d && c != d;
                if distinct && crosses(a, b, c, d) {
                    found.insert((r1, n1));
                    found.insert((r2, n2));
                }
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    /// A decoded feature as the JSON of the shared vectors writes it.
    fn as_json(feature: &Feature) -> Value {
        let geometry = ["point", "line", "area"][feature.geometry.code() as usize];
        let parts: Vec<Value> = feature
            .parts
            .iter()
            .map(|part| {
                let mut out = json!({"outer": part.outer, "xy": part.xy});
                if let Some(z) = &part.z {
                    out["z"] = z.iter().map(|&h| if h.is_nan() { Value::Null } else { json!(h) }).collect();
                }
                if let Some(plan) = &part.plan {
                    out["along"] = json!(plan.along);
                    out["tangent"] = json!(plan.tangent);
                    out["grade"] = json!(plan.grade);
                    out["bank"] = json!(plan.bank);
                    out["widen"] = json!(plan.widen);
                    out["side"] = json!(plan.side);
                    out["walk"] = json!(plan.walk);
                }
                out
            })
            .collect();
        json!({"kind": feature.kind, "tags": feature.tags, "geometry": geometry, "parts": parts})
    }

    #[test]
    fn the_shared_vectors_round_trip() {
        let vectors = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../specs/vectors");
        let decoded: Value =
            serde_json::from_slice(&std::fs::read(vectors.join("trailview-features.json")).unwrap()).unwrap();
        let encoded = std::fs::read(vectors.join("trailview-features.bin")).unwrap();
        let features = decode(&encoded).unwrap();
        assert_eq!(Value::Array(features.iter().map(as_json).collect()), decoded);
        assert_eq!(encode(&features), encoded);
        assert_eq!(Tile::default().bytes(), [0, 0]);
        assert!(decode(&[0, 0]).unwrap().is_empty());
    }

    fn part(outer: bool, points: Vec<[f64; 2]>) -> Part {
        Part { outer, points, z: None, plan: None }
    }

    #[test]
    fn a_feature_with_an_id_takes_more_tags_at_the_write_in_the_order_of_their_keys() {
        let vectors = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../specs/vectors");
        let features = decode(&std::fs::read(vectors.join("trailview-features.bin")).unwrap()).unwrap();
        let mut tile = Tile::default();
        for (k, f) in features.iter().enumerate() {
            tile.add(&f.kind, &f.tags, f.geometry, &f.parts, (k % 2 == 1).then_some(k as u32));
        }
        assert_eq!(tile.bytes(), encode(&features), "no more tags, the same bytes");
        let more = Tags::from([("levels".to_string(), "D".to_string()), ("~".to_string(), "last".to_string())]);
        let tagged = decode(&tile.bytes_with(&|id| (id == 1).then_some(&more))).unwrap();
        for (k, (a, b)) in features.iter().zip(&tagged).enumerate() {
            let mut want = a.clone();
            if k == 1 {
                want.tags.extend(more.clone());
            }
            assert_eq!(as_json(b), as_json(&want), "feature {k}");
        }
    }

    #[test]
    fn an_island_near_a_curved_shore_does_not_cross_it_at_any_zoom() {
        // A lake whose shore curves 45 m over 2 km, with a 200 m island 8 m off the shore.
        let metres = |x: f64, y: f64| world(8.2 + x / 76_220.0, 46.8 + y / 111_320.0);
        let mut shore: Vec<[f64; 2]> =
            (0..=40).map(|k| 50.0 * k as f64).map(|x| [x, -45.0 * (1.0 - ((x - 1000.0) / 1000.0).powi(2))]).collect();
        shore.extend([[2000.0, 800.0], [0.0, 800.0]]);
        let island = [[900.0, -36.0], [1100.0, -36.0], [1100.0, 164.0], [900.0, 164.0]];
        let lake = [
            part(true, shore.iter().map(|&[x, y]| metres(x, y)).collect()),
            part(false, island.iter().map(|&[x, y]| metres(x, y)).collect()),
        ];
        for zoom in MIN_ZOOM..MAX_ZOOM {
            let tolerance = TOLERANCE / ((1u64 << zoom) as f64 * EXTENT_F);
            let rings: Vec<Vec<[f64; 2]>> =
                simplified(Geometry::Area, &lake, &[vec![], vec![]], tolerance).into_iter().map(|p| p.points).collect();
            let edges: Vec<([f64; 2], [f64; 2])> =
                rings.iter().flat_map(|r| (0..r.len()).map(move |i| (r[i], r[(i + 1) % r.len()]))).collect();
            let crossing = (0..edges.len()).any(|n| {
                edges[n + 1..].iter().any(|&(c, d)| {
                    let (a, b) = edges[n];
                    a != c && a != d && b != c && b != d && crosses(a, b, c, d)
                })
            });
            assert!(rings[0].len() < shore.len(), "zoom {zoom}");
            assert_eq!((rings[1].len(), crossing), (4, false), "zoom {zoom}");
        }
    }

    #[test]
    fn a_segment_that_crosses_a_tile_without_a_vertex_in_it_is_in_that_tile() {
        // The region is one tile of zoom 13; a line runs from far west to far east of it.
        let n = (1u64 << 13) as f64;
        let (column, row) = (4286.0, 2889.0);
        let lon = |x: f64| x / n * 360.0 - 180.0;
        let lat = |y: f64| (std::f64::consts::PI * (1.0 - 2.0 * y / n)).sinh().atan().to_degrees();
        let mut tiles = Tiles::new([lon(column), lat(row + 1.0), lon(column + 1.0), lat(row)]);
        let at = |x: f64, y: f64| [x / n, y / n];
        let line = [part(false, vec![at(column - 3.0, row + 0.5), at(column + 4.0, row + 0.5)])];
        assert!(tiles.add(13, "road", Geometry::Line, &Tags::new(), &line, None));
        let tile = decode(&tiles.tiles[&(13, column as u32, row as u32)].bytes()).unwrap();
        // The cut keeps the segment whole: no vertex is added at the tile's edges.
        let width = 7 * i64::from(EXTENT);
        assert_eq!(tile[0].parts[0].xy[1][0] - tile[0].parts[0].xy[0][0], width);
    }

    #[test]
    fn a_stream_keeps_two_vertices_past_the_segments_that_meet_a_tile_and_a_road_one() {
        let n = (1u64 << 13) as f64;
        let (column, row) = (4286.0, 2889.0);
        let lon = |x: f64| x / n * 360.0 - 180.0;
        let lat = |y: f64| (std::f64::consts::PI * (1.0 - 2.0 * y / n)).sinh().atan().to_degrees();
        // Two segments meet the tile; the vertices around them lie a tile apart.
        let line =
            [part(false, [-3.0, -2.0, -1.0, 0.5, 2.0, 3.0, 4.0].map(|x| [(column + x) / n, (row + 0.5) / n]).to_vec())];
        for (kind, vertices) in [("road", 5), ("stream", 7)] {
            let mut tiles = Tiles::new([lon(column), lat(row + 1.0), lon(column + 1.0), lat(row)]);
            assert!(tiles.add(13, kind, Geometry::Line, &Tags::new(), &line, None));
            let tile = decode(&tiles.tiles[&(13, column as u32, row as u32)].bytes()).unwrap();
            assert_eq!(tile[0].parts[0].xy.len(), vertices, "{kind}");
        }
    }

    #[test]
    fn two_tiles_that_hold_a_vertex_hold_one_alignment_and_a_low_zoom_holds_none() {
        let n = (1u64 << MAX_ZOOM) as f64;
        let (column, row) = (4286.0, 2889.0);
        let lon = |x: f64| x / n * 360.0 - 180.0;
        let lat = |y: f64| (std::f64::consts::PI * (1.0 - 2.0 * y / n)).sinh().atan().to_degrees();
        let mut tiles = Tiles::new([lon(column), lat(row + 1.0), lon(column + 2.0), lat(row)]);
        // A line over the border of the two tiles, with a vertex every 2 % of a tile.
        let count = 40;
        let at = |k: f64| [(column + 0.6 + k * 0.02) / n, (row + 0.5 + 0.001 * k * k / 40.0) / n];
        let plan: Vec<Plan> = (0..count)
            .map(|k| {
                let k = k as f64;
                Plan {
                    tangent: 0.5 + k * 0.03,
                    grade: -0.04 + k * 0.002,
                    bank: 0.03 - k * 0.001,
                    widen: k * 0.02,
                    along: 3000.0 - k * 60.0,
                    whole: k == 2.0,
                    side: (k as u8) % 4,
                    walk: (k as u8 / 4) % 4,
                }
            })
            .collect();
        let line = Part {
            points: (0..count).map(|k| at(k as f64)).collect(),
            z: Some(vec![1000.0; count]),
            plan: Some(plan),
            ..Part::default()
        };
        assert!(tiles.add(MAX_ZOOM, "road", Geometry::Line, &Tags::new(), std::slice::from_ref(&line), None));
        let tile =
            |c: f64| decode(&tiles.tiles[&(MAX_ZOOM, c as u32, row as u32)].bytes()).unwrap().remove(0).parts.remove(0);
        let (west, east) = (tile(column), tile(column + 1.0));
        let unit = EXTENT as i64 * FINE;
        let at_east = |p: &[i64; 2]| [p[0] - unit, p[1]];
        let (west_plan, east_plan) = (west.plan.as_ref().unwrap(), east.plan.as_ref().unwrap());
        let shared: Vec<(usize, usize)> = west
            .xy
            .iter()
            .enumerate()
            .filter_map(|(i, p)| Some((i, east.xy.iter().position(|q| *q == at_east(p))?)))
            .collect();
        assert!(shared.len() >= 3, "the buffer of 40 m holds {} vertices of both", shared.len());
        for (i, k) in shared {
            let codes = |plan: &Aligned, v: usize| {
                (plan.tangent[v], plan.grade[v], plan.bank[v], plan.widen[v], plan.side[v], plan.walk[v])
            };
            assert_eq!(codes(west_plan, i), codes(east_plan, k));
            assert_eq!(west.z.as_ref().unwrap()[i], east.z.as_ref().unwrap()[k]);
        }
        // The vertex that a river shares lies on a whole tile unit, as the river's does.
        assert!(west.xy[2].iter().all(|c| c % FINE == 0));
        assert!(west.xy[3].iter().any(|c| c % FINE != 0));
        // The metres along fall along this line. Each piece names them at its own first vertex.
        assert_eq!(west_plan.along, 3000 * 10 * 2 + 1);
        assert_eq!(west_plan.along & 1, east_plan.along & 1);
        let low = simplified(Geometry::Line, std::slice::from_ref(&line), &[vec![]], TOLERANCE / (n * EXTENT_F));
        assert!(low[0].plan.is_none());
    }

    #[test]
    fn a_ring_that_quantizes_to_no_area_goes_with_its_holes() {
        let thin = |outer| part(outer, vec![[10.2, 10.0], [10.4, 10.1], [10.3, 10.3]]);
        let square = |outer, size: f64| part(outer, vec![[0.0, 0.0], [size, 0.0], [size, size], [0.0, size]]);
        let cut = quantized(Geometry::Area, &[thin(true), square(false, 5.0), square(true, 20.0), thin(false)], 0, 0);
        assert_eq!(cut.len(), 1);
        assert_eq!(cut[0].xy, [[0, 0], [20, 0], [20, 20], [0, 20]]);
        // A closed ring drops its repeated first point; a line keeps two points or goes.
        let closed = part(true, vec![[0.0, 0.0], [9.0, 0.0], [9.0, 9.0], [0.0, 0.0]]);
        assert_eq!(quantized(Geometry::Area, &[closed], 0, 0)[0].xy.len(), 3);
        assert!(quantized(Geometry::Line, &[part(false, vec![[1.2, 1.0], [0.9, 1.4]])], 0, 0).is_empty());
    }
}
