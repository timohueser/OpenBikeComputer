//! The reader of the feature tiles of format 4 (`specs/trailview-features.md`): the raw codes of a
//! tile, its features in the frame, the owner of each element of a cell's tiles, and a decode cache.
//!
//! The tiles hold no OSM id. A segment's key is (kind, tags, its two global end points): the same in
//! every tile that holds it, because the cut keeps a segment whole.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use core::f64::consts::TAU;

use crate::frame::{tile_point, GlobalPoint, FEATURE_ZOOM, FRAME_BITS};

pub type Tags = BTreeMap<String, String>;

const TURN: i64 = 1024;
const GRADE: f64 = 250.0;
const BANK: f64 = 1000.0;
const TILE_UNITS: i64 = 1 << (FRAME_BITS - FEATURE_ZOOM);
/// The largest coordinate that a tile may hold, in its units or quarters: far past any cut end, and
/// small enough that every frame point and every difference of two stays far inside `i64`.
const COORDINATE_MAX: i64 = 1 << 40;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Geometry {
    Point,
    Line,
    Area,
}

/// What broke in a tile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// A number or a string runs past the end.
    End,
    /// A varint of more than 10 bytes, or past 64 bits.
    LongVarint,
    /// A string index past the string table.
    StringIndex(u64),
    /// A geometry code other than 0, 1 and 2.
    Geometry(u64),
    InvalidUtf8,
    /// A coordinate past `COORDINATE_MAX` tile units: no bake writes one.
    Coordinate,
    /// Bytes after the last feature: the count of them.
    Trailing(usize),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            DecodeError::End => write!(f, "a feature tile ends inside a number or a string"),
            DecodeError::LongVarint => write!(f, "a feature tile has a varint past 64 bits"),
            DecodeError::StringIndex(index) => write!(f, "a feature tile names string {index} past its table"),
            DecodeError::Geometry(code) => write!(f, "a feature tile has the unknown geometry {code}"),
            DecodeError::InvalidUtf8 => write!(f, "a feature tile has a string that is not UTF-8"),
            DecodeError::Coordinate => write!(f, "a feature tile has a coordinate past 2^40 tile units"),
            DecodeError::Trailing(count) => write!(f, "a feature tile has {count} bytes after its last feature"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// A feature in the codes of the format. `fine` is `has_plan`: the coordinates are quarters of a
/// tile unit, also in a part without an alignment.
#[derive(Clone, Debug, PartialEq)]
pub struct RawFeature {
    pub kind: String,
    pub tags: Tags,
    pub geometry: Geometry,
    pub fine: bool,
    pub parts: Vec<RawPart>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RawPart {
    pub outer: bool,
    pub xy: Vec<[i64; 2]>,
    /// Centimetres per vertex, None where unknown; None for a feature without heights.
    pub z: Option<Vec<Option<i64>>>,
    pub plan: Option<RawPlan>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RawPlan {
    /// `along_code − 1`: 2 × decimetres along the stroke + 1 when it runs against its stroke.
    pub along: u64,
    /// 1/1024 of a turn, 0 to 1023.
    pub tangent: Vec<i64>,
    /// 1/250.
    pub grade: Vec<i64>,
    /// 1/1000.
    pub bank: Vec<i64>,
    /// Centimetres.
    pub widen: Vec<u64>,
    pub side: Vec<u8>,
    pub walk: Vec<u8>,
}

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn varint(&mut self) -> Result<u64, DecodeError> {
        let mut value = 0u64;
        for shift in (0..70).step_by(7) {
            let byte = *self.data.get(self.at).ok_or(DecodeError::End)?;
            self.at += 1;
            let bits = u64::from(byte & 0x7f);
            if shift == 63 && bits > 1 {
                return Err(DecodeError::LongVarint);
            }
            value |= bits << shift;
            if byte < 0x80 {
                return Ok(value);
            }
        }
        Err(DecodeError::LongVarint)
    }

    fn signed(&mut self) -> Result<i64, DecodeError> {
        self.varint().map(zigzag)
    }
}

fn zigzag(value: u64) -> i64 {
    (value >> 1) as i64 ^ -((value & 1) as i64)
}

/// The features of a tile's bytes after gzip.
pub fn decode(bytes: &[u8]) -> Result<Vec<RawFeature>, DecodeError> {
    let mut r = Reader { data: bytes, at: 0 };
    let mut table = Vec::new();
    for _ in 0..r.varint()? {
        let length = usize::try_from(r.varint()?).map_err(|_| DecodeError::End)?;
        let end = r.at.checked_add(length).ok_or(DecodeError::End)?;
        let text = bytes.get(r.at..end).ok_or(DecodeError::End)?;
        table.push(std::str::from_utf8(text).map_err(|_| DecodeError::InvalidUtf8)?.to_string());
        r.at = end;
    }
    let text = |index: u64| {
        usize::try_from(index).ok().and_then(|i| table.get(i)).cloned().ok_or(DecodeError::StringIndex(index))
    };
    let mut heads = Vec::new();
    for _ in 0..r.varint()? {
        let kind = text(r.varint()?)?;
        heads.push((kind, r.varint()?));
    }
    let mut features = Vec::with_capacity(heads.len());
    for (kind, header) in &heads {
        let mut tags = Tags::new();
        for _ in 0..r.varint()? {
            let key = text(r.varint()?)?;
            tags.insert(key, text(r.varint()?)?);
        }
        let geometry = match header & 3 {
            0 => Geometry::Point,
            1 => Geometry::Line,
            2 => Geometry::Area,
            code => return Err(DecodeError::Geometry(code)),
        };
        features.push(RawFeature { kind: kind.clone(), tags, geometry, fine: header & 8 != 0, parts: Vec::new() });
    }
    // The vertex count of each part: the streams after this one do not name it.
    let mut counts: Vec<Vec<u64>> = Vec::with_capacity(heads.len());
    for (feature, (_, header)) in features.iter_mut().zip(&heads) {
        let solved = header & 4 != 0;
        let mut sizes = Vec::new();
        for _ in 0..header >> 4 {
            let code = r.varint()?;
            sizes.push(code >> 1);
            feature.parts.push(RawPart { outer: code & 1 == 1, xy: Vec::new(), z: solved.then(Vec::new), plan: None });
        }
        counts.push(sizes);
    }
    for (feature, sizes) in features.iter_mut().zip(&counts) {
        let (mut x, mut y, mut height) = (0i64, 0i64, 0i64);
        for (part, &size) in feature.parts.iter_mut().zip(sizes) {
            for _ in 0..size {
                x = x.checked_add(r.signed()?).filter(|v| v.abs() <= COORDINATE_MAX).ok_or(DecodeError::Coordinate)?;
                y = y.checked_add(r.signed()?).filter(|v| v.abs() <= COORDINATE_MAX).ok_or(DecodeError::Coordinate)?;
                part.xy.push([x, y]);
                if let Some(z) = &mut part.z {
                    let code = r.varint()?;
                    z.push((code != 0).then(|| {
                        height = height.wrapping_add(zigzag(code - 1));
                        height
                    }));
                }
            }
        }
    }
    // (feature, part, along_code) of each part with an alignment.
    let mut aligned = Vec::new();
    for (f, (_, header)) in heads.iter().enumerate() {
        if header & 8 == 0 {
            continue;
        }
        for p in 0..counts[f].len() {
            let code = r.varint()?;
            if code != 0 {
                aligned.push((f, p, code));
            }
        }
    }
    let column = |r: &mut Reader| -> Result<Vec<Vec<u64>>, DecodeError> {
        aligned.iter().map(|&(f, p, _)| (0..counts[f][p]).map(|_| r.varint()).collect()).collect()
    };
    let (tangents, grades) = (column(&mut r)?, column(&mut r)?);
    let (banks, words) = (column(&mut r)?, column(&mut r)?);
    let running = |codes: &[u64]| -> Vec<i64> {
        let mut sum = 0i64;
        codes
            .iter()
            .map(|&code| {
                sum = sum.wrapping_add(zigzag(code));
                sum
            })
            .collect()
    };
    for (n, &(f, p, code)) in aligned.iter().enumerate() {
        features[f].parts[p].plan = Some(RawPlan {
            along: code - 1,
            tangent: running(&tangents[n]).iter().map(|turn| turn.rem_euclid(TURN)).collect(),
            grade: running(&grades[n]),
            bank: banks[n].iter().map(|&code| zigzag(code)).collect(),
            widen: words[n].iter().map(|word| word >> 4).collect(),
            side: words[n].iter().map(|word| (word & 3) as u8).collect(),
            walk: words[n].iter().map(|word| (word >> 2 & 3) as u8).collect(),
        });
    }
    if r.at != bytes.len() {
        return Err(DecodeError::Trailing(bytes.len() - r.at));
    }
    Ok(features)
}

/// A feature of a zoom-13 tile in the frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Feature {
    /// The feature's position in its tile.
    pub index: u32,
    pub kind: String,
    pub tags: Tags,
    pub geometry: Geometry,
    pub parts: Vec<Part>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub outer: bool,
    /// The zoom-13 tile (column, row) that this copy comes from.
    pub tile: (u32, u32),
    /// Per segment (line), edge (ring, the last vertex to the first included) or point: whether this
    /// copy's tile owns it. Empty until a cell sets it.
    pub own: Vec<bool>,
    pub points: Vec<GlobalPoint>,
    /// Metres per vertex, None where unknown; None for a feature without heights.
    pub heights: Option<Vec<Option<f64>>>,
    pub plan: Option<Alignment>,
}

/// The alignment of a part, per vertex in the direction of the part.
#[derive(Clone, Debug, PartialEq)]
pub struct Alignment {
    /// Metres along the stroke at the first vertex. Two tiles round it differently by up to 10 cm.
    pub along_m: f64,
    /// The metres along fall along the part: it runs against its stroke.
    pub backward: bool,
    /// Radians from +x to +y (east to south), 0 to τ.
    pub tangent: Vec<f64>,
    /// Rise per metre.
    pub grade: Vec<f64>,
    /// Rise per metre to the left of the direction.
    pub bank: Vec<f64>,
    /// Metres that the half-width gains.
    pub widen: Vec<f64>,
    /// The ways out that leave the vertex's junction as a side: 1 back along the part, 2 on.
    pub side: Vec<u8>,
    /// The sides of the segment from the vertex on with a sidewalk: 1 left, 2 right. 0 at the last vertex.
    pub walk: Vec<u8>,
}

/// The features of a zoom-13 tile's bytes after gzip, in the tile's order, with no vertex changed.
pub fn features(bytes: &[u8], column: u32, row: u32) -> Result<Vec<Feature>, DecodeError> {
    let raw = decode(bytes)?;
    Ok(raw
        .into_iter()
        .enumerate()
        .map(|(index, feature)| {
            let fine = feature.fine;
            let parts = feature
                .parts
                .into_iter()
                .map(|part| Part {
                    outer: part.outer,
                    tile: (column, row),
                    own: Vec::new(),
                    points: part.xy.iter().map(|&[x, y]| tile_point(FEATURE_ZOOM, column, row, fine, x, y)).collect(),
                    heights: part.z.map(|z| z.into_iter().map(|cm| cm.map(|cm| cm as f64 / 100.0)).collect()),
                    plan: part.plan.map(alignment),
                })
                .collect();
            Feature { index: index as u32, kind: feature.kind, tags: feature.tags, geometry: feature.geometry, parts }
        })
        .collect())
}

fn alignment(plan: RawPlan) -> Alignment {
    let mut walk = plan.walk;
    // No segment starts at the last vertex: the cut can leave its walk set.
    if let Some(last) = walk.last_mut() {
        *last = 0;
    }
    Alignment {
        along_m: (plan.along >> 1) as f64 / 10.0,
        backward: plan.along & 1 == 1,
        tangent: plan.tangent.iter().map(|&code| code as f64 * TAU / TURN as f64).collect(),
        grade: plan.grade.iter().map(|&code| code as f64 / GRADE).collect(),
        bank: plan.bank.iter().map(|&code| code as f64 / BANK).collect(),
        widen: plan.widen.iter().map(|&cm| cm as f64 / 100.0).collect(),
        side: plan.side,
        walk,
    }
}

/// The closed square [x0, y0, x1, y1] of a zoom-13 tile.
pub fn tile_square((column, row): (u32, u32)) -> [i64; 4] {
    let (x, y) = (i64::from(column) * TILE_UNITS, i64::from(row) * TILE_UNITS);
    [x, y, x + TILE_UNITS, y + TILE_UNITS]
}

/// Whether the segment a–b meets the closed box [x0, y0, x1, y1]: separating axes, exact in `i128`
/// for coordinates within ±2^62, which holds every decoded point (`COORDINATE_MAX`).
pub fn segment_meets(a: GlobalPoint, b: GlobalPoint, [x0, y0, x1, y1]: [i64; 4]) -> bool {
    if a.x.max(b.x) < x0 || a.x.min(b.x) > x1 || a.y.max(b.y) < y0 || a.y.min(b.y) > y1 {
        return false;
    }
    let d = |p: i64, q: i64| i128::from(p) - i128::from(q);
    let (dx, dy) = (d(b.x, a.x), d(b.y, a.y));
    let side = |x: i64, y: i64| (dx * d(y, a.y) - dy * d(x, a.x)).signum();
    let sides = [side(x0, y0), side(x1, y0), side(x0, y1), side(x1, y1)];
    !(sides.iter().all(|&s| s > 0) || sides.iter().all(|&s| s < 0))
}

/// The element `i` of a part: a point, a segment, or a ring edge from the last vertex to the first.
fn element(geometry: Geometry, points: &[GlobalPoint], i: usize) -> (GlobalPoint, GlobalPoint) {
    match geometry {
        Geometry::Point => (points[i], points[i]),
        Geometry::Line => (points[i], points[i + 1]),
        Geometry::Area => (points[i], points[(i + 1) % points.len()]),
    }
}

/// The elements of a part: its points, segments or ring edges.
pub fn element_count(geometry: Geometry, points: usize) -> usize {
    match geometry {
        Geometry::Point | Geometry::Area => points,
        Geometry::Line => points.saturating_sub(1),
    }
}

/// The index in `tiles` of the owner of an element: the first tile whose square it meets. A point
/// meets the half-open square, so a point on a tile line belongs to the tile east or south of it;
/// a segment meets the closed square.
pub fn owner(tiles: &[(u32, u32)], a: GlobalPoint, b: GlobalPoint) -> Option<usize> {
    tiles.iter().position(|&tile| {
        let [x0, y0, x1, y1] = tile_square(tile);
        if a == b {
            (x0..x1).contains(&a.x) && (y0..y1).contains(&a.y)
        } else {
            segment_meets(a, b, [x0, y0, x1, y1])
        }
    })
}

/// The owner flags of each element of a part of the tile at `tiles[at]`.
pub fn own(geometry: Geometry, points: &[GlobalPoint], tiles: &[(u32, u32)], at: usize) -> Vec<bool> {
    (0..element_count(geometry, points.len()))
        .map(|i| {
            let (a, b) = element(geometry, points, i);
            owner(tiles, a, b) == Some(at)
        })
        .collect()
}

/// A decoded tile and the clock of its last use.
type Cached = (Arc<Vec<Feature>>, u64);

/// The decoded features of the tiles used last, without `own`. A decode is pure, so the cache
/// changes no output byte.
pub struct TileCache {
    capacity: usize,
    clock: u64,
    tiles: BTreeMap<(u32, u32), Cached>,
}

impl TileCache {
    pub fn new(capacity: usize) -> TileCache {
        TileCache { capacity: capacity.max(1), clock: 0, tiles: BTreeMap::new() }
    }

    /// The features of tile (column, row). `load` gives its bytes after gzip, or None where the
    /// archive does not hold it: no features.
    pub fn tile(
        &mut self,
        column: u32,
        row: u32,
        load: impl FnOnce() -> Option<Vec<u8>>,
    ) -> Result<Arc<Vec<Feature>>, DecodeError> {
        self.clock += 1;
        if let Some((features, used)) = self.tiles.get_mut(&(column, row)) {
            *used = self.clock;
            return Ok(features.clone());
        }
        let decoded = Arc::new(match load() {
            Some(bytes) => features(&bytes, column, row)?,
            None => Vec::new(),
        });
        self.tiles.insert((column, row), (decoded.clone(), self.clock));
        if self.tiles.len() > self.capacity {
            let oldest = self.tiles.iter().min_by_key(|(_, (_, used))| *used).map(|(&key, _)| key);
            oldest.map(|key| self.tiles.remove(&key));
        }
        Ok(decoded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn vectors() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../specs/vectors")
    }

    fn as_json(feature: &RawFeature) -> Value {
        let geometry = ["point", "line", "area"][feature.geometry as usize];
        let parts: Vec<Value> = feature
            .parts
            .iter()
            .map(|part| {
                let mut out = json!({"outer": part.outer, "xy": part.xy});
                if let Some(z) = &part.z {
                    out["z"] = z.iter().map(|h| h.map_or(Value::Null, |cm| json!(cm as f64 / 100.0))).collect();
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
    fn the_shared_vectors_decode() {
        let decoded: Value =
            serde_json::from_slice(&std::fs::read(vectors().join("trailview-features.json")).unwrap()).unwrap();
        let features = decode(&std::fs::read(vectors().join("trailview-features.bin")).unwrap()).unwrap();
        assert_eq!(Value::Array(features.iter().map(as_json).collect()), decoded);
        assert!(decode(&[0, 0]).unwrap().is_empty());
    }

    #[test]
    fn a_broken_tile_names_what_broke() {
        let bytes = std::fs::read(vectors().join("trailview-features.bin")).unwrap();
        assert_eq!(decode(&bytes[..bytes.len() - 1]), Err(DecodeError::End));
        assert_eq!(decode(&[bytes.as_slice(), &[0]].concat()), Err(DecodeError::Trailing(1)));
        // One string, one feature whose kind is string 5.
        assert_eq!(decode(&[1, 1, b'a', 1, 5, 0x11]), Err(DecodeError::StringIndex(5)));
        assert_eq!(decode(&[1, 1, b'a', 1, 0, 0x13, 0]), Err(DecodeError::Geometry(3)));
        assert_eq!(decode(&[1, 1, 0xff, 0]), Err(DecodeError::InvalidUtf8));
        assert_eq!(decode(&[0x80; 11]), Err(DecodeError::LongVarint));
        // One line of two vertices whose first dx is i64::MAX: an error, not an overflow.
        let far = [
            &[1, 1, b'a', 1, 0, 0x11, 0, 4][..],
            &[0xfe, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01],
            &[0, 0, 0],
        ]
        .concat();
        assert_eq!(decode(&far), Err(DecodeError::Coordinate));
        assert_eq!(features(&far, 4286, 2888), Err(DecodeError::Coordinate));
    }

    #[test]
    fn a_quarter_and_a_whole_unit_give_one_point_and_the_last_walk_is_0() {
        let bytes = std::fs::read(vectors().join("trailview-features.bin")).unwrap();
        let raw = decode(&bytes).unwrap();
        let framed = features(&bytes, 4286, 2888).unwrap();
        for (feature, raw) in framed.iter().zip(&raw) {
            let (fine, origin) = (if raw.fine { 4 } else { 1 }, tile_point(13, 4286, 2888, false, 0, 0));
            for (part, raw) in feature.parts.iter().zip(&raw.parts) {
                for (p, &[x, y]) in part.points.iter().zip(&raw.xy) {
                    assert_eq!(*p, GlobalPoint { x: origin.x + x * 128 / fine, y: origin.y + y * 128 / fine });
                }
                if let Some(plan) = &part.plan {
                    assert_eq!(plan.walk.last(), Some(&0));
                    assert_eq!(
                        &plan.walk[..plan.walk.len() - 1],
                        &raw.plan.as_ref().unwrap().walk[..plan.walk.len() - 1]
                    );
                }
            }
        }
        // The tertiary road counts in quarters: its vertex -601 / 4 lies 150.25 units west of the tile.
        let tertiary = &framed[2].parts[0];
        assert_eq!(tertiary.points[0].x - tile_point(13, 4286, 2888, false, 0, 0).x, -601 * 32);
        assert_eq!(tile_point(13, 4286, 2888, true, 400, 8), tile_point(13, 4286, 2888, false, 100, 2));
    }

    #[test]
    fn a_segment_meets_a_box_exactly() {
        let p = |x, y| GlobalPoint { x, y };
        let square = [0, 0, 10, 10];
        assert!(segment_meets(p(-5, 5), p(15, 5), square));
        assert!(segment_meets(p(10, 10), p(20, 0), square), "a corner");
        assert!(!segment_meets(p(11, 10), p(20, 1), square), "past the corner");
        assert!(!segment_meets(p(-10, 9), p(9, 28), square), "a diagonal past the corner");
        // A point on a tile line belongs to the tile east of it.
        let tiles = [(0, 0), (1, 0)];
        assert_eq!(owner(&tiles, p(TILE_UNITS, 5), p(TILE_UNITS, 5)), Some(1));
        assert_eq!(owner(&tiles, p(TILE_UNITS - 3, 5), p(TILE_UNITS + 3, 5)), Some(0));
    }
}
