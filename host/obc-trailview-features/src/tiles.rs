//! The tiles of the archive: the features cut to the buffered square of each tile, in tile units,
//! and their encoding (`features_format` of `specs/trailview-features.md`).

use std::collections::{BTreeSet, HashMap};

use crate::{world, Part, Tags, EQUATOR_M};

pub const FORMAT: u32 = 2;
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

/// A part in integer tile units, with a height per point (NaN where unknown) or none.
#[derive(Clone, Debug, PartialEq)]
pub struct Quantized {
    pub outer: bool,
    pub xy: Vec<[i64; 2]>,
    pub z: Option<Vec<f64>>,
}

/// A feature of a tile, as `decode` gives it and `encode` takes it.
#[derive(Clone, Debug, PartialEq)]
pub struct Feature {
    pub kind: String,
    pub tags: Tags,
    pub geometry: Geometry,
    pub parts: Vec<Quantized>,
}

/// The encoder of one tile.
#[derive(Default)]
pub struct Tile {
    strings: HashMap<String, u64>,
    order: Vec<String>,
    body: Vec<u8>,
    count: u64,
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

    /// Add a feature. A height is in metres; a part without heights in a feature with some has an
    /// unknown height at each point.
    pub fn add(&mut self, kind: &str, tags: &Tags, geometry: Geometry, parts: &[Quantized]) {
        let mut head = Vec::new();
        varint(&mut head, self.string(kind));
        varint(&mut head, tags.len() as u64);
        for (key, value) in tags {
            varint(&mut head, self.string(key));
            varint(&mut head, self.string(value));
        }
        self.body.extend(head);
        let solved = parts.iter().any(|part| part.z.is_some());
        varint(&mut self.body, (parts.len() as u64) << 3 | u64::from(solved) << 2 | geometry.code());
        let (mut x, mut y, mut height) = (0, 0, 0i64);
        for part in parts {
            varint(&mut self.body, (part.xy.len() as u64) << 1 | u64::from(part.outer));
            for (index, &[px, py]) in part.xy.iter().enumerate() {
                varint(&mut self.body, zigzag(px - x));
                varint(&mut self.body, zigzag(py - y));
                (x, y) = (px, py);
                if !solved {
                    continue;
                }
                match part.z.as_ref().map(|z| z[index]).filter(|value| !value.is_nan()) {
                    None => self.body.push(0),
                    Some(value) => {
                        let value = (value * 100.0).round_ties_even() as i64;
                        varint(&mut self.body, zigzag(value - height) + 1);
                        height = value;
                    }
                }
            }
        }
        self.count += 1;
    }

    /// The bytes of the tile.
    pub fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        varint(&mut out, self.order.len() as u64);
        for text in &self.order {
            varint(&mut out, text.len() as u64);
            out.extend(text.as_bytes());
        }
        varint(&mut out, self.count);
        out.extend(&self.body);
        out
    }
}

/// The bytes of a tile that holds `features`.
pub fn encode(features: &[Feature]) -> Vec<u8> {
    let mut tile = Tile::default();
    for feature in features {
        tile.add(&feature.kind, &feature.tags, feature.geometry, &feature.parts);
    }
    tile.bytes()
}

/// The features of a tile.
pub fn decode(data: &[u8]) -> Result<Vec<Feature>, String> {
    let next = |at: &mut usize| -> Result<u64, String> {
        let (mut value, mut shift) = (0u64, 0);
        loop {
            let byte = *data.get(*at).ok_or("a feature tile ends inside a varint")?;
            *at += 1;
            value |= u64::from(byte & 0x7f) << shift;
            shift += 7;
            if byte < 0x80 {
                return Ok(value);
            }
        }
    };
    let signed = |value: u64| (value >> 1) as i64 ^ -((value & 1) as i64);
    let mut at = 0;
    let mut table = Vec::new();
    for _ in 0..next(&mut at)? {
        let length = next(&mut at)? as usize;
        let text = data.get(at..at + length).ok_or("a feature tile ends inside a string")?;
        table.push(String::from_utf8(text.to_vec()).map_err(|e| e.to_string())?);
        at += length;
    }
    let text = |index: u64| table.get(index as usize).cloned().ok_or("a string index past the table");
    let mut features = Vec::new();
    for _ in 0..next(&mut at)? {
        let kind = text(next(&mut at)?)?;
        let mut tags = Tags::new();
        for _ in 0..next(&mut at)? {
            let key = text(next(&mut at)?)?;
            tags.insert(key, text(next(&mut at)?)?);
        }
        let header = next(&mut at)?;
        let solved = header & 4 != 0;
        let geometry = [Geometry::Point, Geometry::Line, Geometry::Area]
            .get((header & 3) as usize)
            .copied()
            .ok_or("an unknown geometry")?;
        let (mut x, mut y, mut height) = (0i64, 0i64, 0i64);
        let mut parts = Vec::new();
        for _ in 0..header >> 3 {
            let count = next(&mut at)?;
            let mut part = Quantized { outer: count & 1 == 1, xy: Vec::new(), z: solved.then(Vec::new) };
            for _ in 0..count >> 1 {
                x += signed(next(&mut at)?);
                y += signed(next(&mut at)?);
                part.xy.push([x, y]);
                if let Some(z) = &mut part.z {
                    let code = next(&mut at)?;
                    if code == 0 {
                        z.push(f64::NAN);
                    } else {
                        height += signed(code - 1);
                        z.push(height as f64 / 100.0);
                    }
                }
            }
            parts.push(part);
        }
        features.push(Feature { kind, tags, geometry, parts });
    }
    if at != data.len() {
        return Err("a feature tile has bytes after its last feature".into());
    }
    Ok(features)
}

/// Python's `a // b` of floats: the floor of the exact quotient, not of the rounded one.
fn floor_divide(a: f64, b: f64) -> f64 {
    let modulo = a % b;
    let mut div = (a - modulo) / b;
    if modulo != 0.0 && ((b < 0.0) != (modulo < 0.0)) {
        div -= 1.0;
    }
    if div == 0.0 {
        return 0.0f64.copysign(a / b);
    }
    let floor = div.floor();
    if div - floor > 0.5 {
        floor + 1.0
    } else {
        floor
    }
}

/// The box (x0, y0, x1, y1) of the points of the parts, or None when they have none.
fn extent(parts: &[Part]) -> Option<[f64; 4]> {
    let mut points = parts.iter().flat_map(|part| part.points.iter());
    let &[x, y] = points.next()?;
    Some(points.fold([x, y, x, y], |b, &[x, y]| [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)]))
}

/// What a tile keeps of a feature besides its parts.
#[derive(Clone, Copy)]
struct Named<'a> {
    kind: &'a str,
    geometry: Geometry,
    tags: &'a Tags,
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
    /// that square. Whether a tile took it.
    pub fn add(&mut self, zoom: u8, kind: &str, geometry: Geometry, tags: &Tags, parts: &[Part]) -> bool {
        let scale = (1u64 << zoom) as f64 * EXTENT_F;
        let parts: Vec<Part> = parts
            .iter()
            .map(|part| Part {
                outer: part.outer,
                points: part.points.iter().map(|&[x, y]| [x * scale, y * scale]).collect(),
                z: part.z.clone(),
            })
            .collect();
        let Some(box_) = extent(&parts) else { return false };
        let row = |value: f64| (value / EXTENT_F).floor() as i64;
        // The buffer grows towards the pole: twice that of the outer rows bounds every row's.
        let margin = 2.0 * self.pad(zoom, row(box_[1]) - 1).max(self.pad(zoom, row(box_[3]) + 1));
        let [c0, r0, c1, r1] = self.span(zoom);
        let (c0, r0) = (c0.max(row(box_[0] - margin)), r0.max(row(box_[1] - margin)));
        let (c1, r1) = (c1.min(row(box_[2] + margin)), r1.min(row(box_[3] + margin)));
        let feature = Named { kind, geometry, tags };
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
        let Named { kind, geometry, tags } = *feature;
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
            parts = clipped(geometry, &parts, rect);
            match extent(&parts) {
                Some(found) => box_ = found,
                None => return false,
            }
        }
        if (c0, r0) == (c1, r1) {
            let cut = quantized(geometry, &parts, c0 * extent_i, r0 * extent_i);
            if !cut.is_empty() {
                self.tiles.entry((zoom, c0 as u32, r0 as u32)).or_default().add(kind, tags, geometry, &cut);
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
/// that meet `rect` whole, and one more vertex at each end: the tiles that hold a segment hold the
/// same segment and the same bends at its ends.
fn clip_line(points: &[[f64; 2]], rect: [f64; 4]) -> Vec<Vec<usize>> {
    let mut keep = vec![false; points.len()];
    for k in 0..points.len().saturating_sub(1) {
        if meets(points[k], points[k + 1], rect) {
            keep[k.saturating_sub(1)..(k + 3).min(points.len())].fill(true);
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

/// Parts cut to `rect`. The cut makes no new vertex.
fn clipped(geometry: Geometry, parts: &[Part], rect: [f64; 4]) -> Vec<Part> {
    let pick = |outer: bool, part: &Part, indices: &[usize]| Part {
        outer,
        points: indices.iter().map(|&i| part.points[i]).collect(),
        z: part.z.as_ref().map(|z| indices.iter().map(|&i| z[i]).collect()),
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
            .flat_map(|part| clip_line(&part.points, rect).into_iter().map(move |piece| (part, piece)))
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

/// Parts in integer tile units. A part without enough distinct points goes, and the holes of an
/// outer ring that goes go with it.
fn quantized(geometry: Geometry, parts: &[Part], left: i64, top: i64) -> Vec<Quantized> {
    let (mut result, mut kept) = (Vec::new(), false);
    for part in parts {
        // A part with no heights, or an empty list of them, has none.
        let z = part.z.as_ref().filter(|z| !z.is_empty());
        let (mut xy, mut heights): (Vec<[i64; 2]>, Vec<f64>) = (Vec::new(), Vec::new());
        for (n, &[x, y]) in part.points.iter().enumerate() {
            let point = [x.round_ties_even() as i64 - left, y.round_ties_even() as i64 - top];
            if xy.last() != Some(&point) {
                xy.push(point);
                heights.push(z.map_or(f64::NAN, |z| z[n]));
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
        result.push(Quantized { outer: part.outer, xy, z: z.map(|_| heights) });
    }
    result
}

/// Douglas-Peucker on each part. The `anchors` of each part, the indices of vertices that other
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
            }
        })
        .collect()
}

/// Python's `math.hypot` of two values.
fn hypot(x: f64, y: f64) -> f64 {
    x.hypot(y)
}

/// The index between i and j of the point farthest from the chord i-j, and its distance.
fn farthest(points: &[[f64; 2]], i: usize, j: usize) -> (Option<usize>, f64) {
    let ([ax, ay], [bx, by]) = (points[i], points[j]);
    let (dx, dy) = (bx - ax, by - ay);
    let length = hypot(dx, dy);
    let (mut distance, mut index) = (-1.0, None);
    for (k, &[px, py]) in points.iter().enumerate().take(j).skip(i + 1) {
        let d = if length != 0.0 { (dx * (ay - py) - dy * (ax - px)).abs() / length } else { hypot(px - ax, py - ay) };
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
        let index = |v: f64| floor_divide(v, cell) as i64;
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
    }

    fn part(outer: bool, points: Vec<[f64; 2]>) -> Part {
        Part { outer, points, z: None }
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
        assert!(tiles.add(13, "road", Geometry::Line, &Tags::new(), &line));
        let tile = decode(&tiles.tiles[&(13, column as u32, row as u32)].bytes()).unwrap();
        // The cut keeps the segment whole: no vertex is added at the tile's edges.
        let width = 7 * i64::from(EXTENT);
        assert_eq!(tile[0].parts[0].xy[1][0] - tile[0].parts[0].xy[0][0], width);
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
