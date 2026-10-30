//! The TrailView feature tiles of a region, the `planner/trailview-features` step of `obc data`
//! ([`step`]): the OSM objects of the kind table ([`kinds`], [`osm`]), with the levels of rivers and
//! lakes ([`water`]), the heights of roads, paths, tracks, bridges and tunnels ([`roads`]) from the
//! master terrain ([`terrain`]), the lifts ([`lifts`]), the railway furniture ([`rails`]), the
//! bridges and tunnels ([`structures`]) and the sea ([`sea`]), cut into tiles ([`tiles`]) of a
//! PMTiles archive ([`pmtiles`]). `specs/trailview-features.md` defines the archive.

use std::collections::BTreeMap;

pub mod bake;
mod geometry;
pub mod kinds;
mod lifts;
mod osm;
mod pmtiles;
mod rails;
mod roads;
mod sea;
pub mod step;
mod structures;
pub mod terrain;
mod tiles;
mod water;

pub const EQUATOR_M: f64 = 40075016.686;

/// Metres per world unit at world row y: the Mercator scale.
pub fn metres(y: f64) -> f64 {
    EQUATOR_M * (std::f64::consts::PI * (1.0 - 2.0 * y)).sinh().atan().cos()
}

/// Web Mercator coordinates in [0, 1], y down.
pub fn world(lon: f64, lat: f64) -> [f64; 2] {
    [(lon + 180.0) / 360.0, (1.0 - lat.to_radians().tan().asinh() / std::f64::consts::PI) / 2.0]
}

/// Heights in metres at world points (x, y in [0, 1], y down); NaN where unknown.
pub trait Terrain: Sync {
    fn heights(&self, points: &[[f64; 2]]) -> Vec<f64>;

    /// The deepest zoom of its tiles.
    fn top(&self) -> u32 {
        13
    }

    fn height(&self, point: [f64; 2]) -> f64 {
        self.heights(&[point])[0]
    }
}

impl<F: Fn(f64, f64) -> f64 + Sync> Terrain for F {
    fn heights(&self, points: &[[f64; 2]]) -> Vec<f64> {
        points.iter().map(|&[x, y]| self(x, y)).collect()
    }
}

/// The OSM tags that a feature keeps.
pub type Tags = BTreeMap<String, String>;

/// The bits of `Plan::side`: the way from a vertex back along its line, and the way on.
pub const SIDE_BACK: u8 = 1;
pub const SIDE_ON: u8 = 2;

/// The alignment of a line at a vertex, in the direction of the line.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Plan {
    /// The heading of the curve in radians: the angle from x to y of world coordinates.
    pub tangent: f64,
    /// Rise per metre.
    pub grade: f64,
    /// Rise per metre to the left.
    pub bank: f64,
    /// Metres that the half-width of a road gains.
    pub widen: f64,
    /// Metres along its stroke. It falls along a way that runs against its stroke.
    pub along: f64,
    /// A vertex that a river or a stream shares: it lies on a whole tile unit, as theirs do.
    pub whole: bool,
    /// The ways out of the vertex that leave its junction as a side: `SIDE_BACK` and `SIDE_ON`.
    pub side: u8,
}

/// A point, a line or a ring of a feature, in world coordinates, with a height per point (NaN where
/// unknown) or none, and the alignment per point of a line that the road solver sets. An outer ring
/// precedes its holes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Part {
    pub outer: bool,
    pub points: Vec<[f64; 2]>,
    pub z: Option<Vec<f64>>,
    pub plan: Option<Vec<Plan>>,
}
