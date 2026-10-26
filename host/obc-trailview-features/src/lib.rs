//! The height solvers of the TrailView feature tiles, the `planner/trailview-features` step of
//! `obc data`: the levels of rivers and lakes ([`water`]) and the heights of roads, paths, tracks,
//! bridges and tunnels ([`roads`]), from the master terrain ([`terrain`]).
//! `specs/trailview-features.md` says what the heights promise. `tools/planner_trailview_features.py`
//! reads, places and tiles the features; it hands the solvers one job file ([`job`]).

pub mod job;
pub mod roads;
pub mod terrain;
pub mod water;

pub const EQUATOR_M: f64 = 40075016.686;

/// Metres per world unit at world row y: the Mercator scale.
pub fn metres(y: f64) -> f64 {
    EQUATOR_M * (std::f64::consts::PI * (1.0 - 2.0 * y)).sinh().atan().cos()
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

/// A line or a ring of a feature, in world coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub outer: bool,
    pub points: Vec<[f64; 2]>,
}
