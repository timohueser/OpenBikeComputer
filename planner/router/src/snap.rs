use crate::{
    geometry,
    model::{Point, Road},
    package::CELL,
    Error, Result,
};
use std::collections::BTreeSet;

/// The snap radius for a point with no road within `Policy::radius_m`, and for a point whose
/// nearest road no route reaches. The client shows the gap. Farther away, the nearest road is often
/// on another ridge or across a valley.
pub const REACH_M: f64 = 1_000.0;

impl Default for Policy {
    fn default() -> Self {
        Self { radius_m: 250.0, ambiguity_m: 3.0, max_candidates: 8 }
    }
}

/// The candidates among `roads`. `project` gives the attachment to a road within `policy.radius_m`,
/// or none for a road that is farther or that the profile cannot use.
pub(crate) fn candidates(
    roads: impl IntoIterator<Item = u32>,
    policy: Policy,
    mut project: impl FnMut(u32) -> Result<Option<Candidate>>,
) -> Result<Candidates> {
    if !policy.ambiguity_m.is_finite()
        || !(0.0..=REACH_M).contains(&policy.ambiguity_m)
        || policy.max_candidates == 0
        || policy.max_candidates > 16
    {
        return Err(Error::InvalidRequest("Invalid snapping policy".into()));
    }
    let mut found = Vec::new();
    for id in roads {
        found.extend(project(id)?);
    }
    found.sort_by(|a, b| a.snap_distance_m.total_cmp(&b.snap_distance_m).then(a.position.road.cmp(&b.position.road)));
    if let Some(nearest) = found.first().map(|c| c.snap_distance_m) {
        found.retain(|c| c.snap_distance_m <= nearest + policy.ambiguity_m);
    }
    let truncated = found.len() > policy.max_candidates;
    found.truncate(policy.max_candidates);
    Ok(Candidates { truncated, retained: found })
}

#[derive(Clone, Copy, Debug)]
pub struct Position {
    pub road: u32,
    /// Fraction of the directed polyline's geometric length, from zero to one.
    pub fraction: f64,
}
impl Position {
    /// The opaque wire form. It keeps the exact fraction, so a pin matches only this position.
    pub fn id(&self) -> String {
        format!("{}:{}", self.road, self.fraction)
    }
}

#[derive(Clone, Debug)]
pub struct Candidate {
    pub position: Position,
    pub projected: Point,
    pub snap_distance_m: f64,
}

#[derive(Clone, Copy, Debug)]
pub struct Policy {
    pub radius_m: f64,
    pub ambiguity_m: f64,
    pub max_candidates: usize,
}

#[derive(Clone, Debug)]
pub struct Candidates {
    pub truncated: bool,
    pub retained: Vec<Candidate>,
}

pub(crate) fn cells(point: Point, radius: f64) -> std::result::Result<BTreeSet<(i32, i32)>, String> {
    if !radius.is_finite()
        || !(0.0..=5_000.0).contains(&radius)
        || point.lat.unsigned_abs() > 85_000_000
        || point.lon.unsigned_abs() > 180_000_000
    {
        return Err("Snap windows require a radius up to 5 km and latitude within ±85 degrees".into());
    }
    let lat = (radius / 0.110).ceil() as i32 + 2;
    let lon = (radius / (0.110 * (point.lat as f64 * 1e-6).to_radians().cos())).ceil() as i32 + 2;
    let mut result = BTreeSet::new();
    for y in (point.lat - lat).div_euclid(CELL)..=(point.lat + lat).div_euclid(CELL) {
        for x in (point.lon - lon).div_euclid(CELL)..=(point.lon + lon).div_euclid(CELL) {
            result.insert((y, x));
        }
    }
    Ok(result)
}

/// The attachment of `point` to road `id`, when the road is within `radius_m`.
pub fn project(id: u32, road: &Road, point: Point, radius_m: f64) -> Option<Candidate> {
    let mut nearest: Option<(usize, f64, f64)> = None;
    for (segment, pair) in road.shape.windows(2).enumerate() {
        let (t, distance) = geometry::project(point, pair[0], pair[1]);
        if nearest.is_none_or(|(.., best)| distance < best) {
            nearest = Some((segment, t, distance));
        }
    }
    let (segment, t, _) = nearest?;
    let (a, b) = (road.shape[segment], road.shape[segment + 1]);
    let projected = geometry::lerp(a, b, t);
    let snap_distance_m = point.distance(projected);
    if snap_distance_m > radius_m {
        return None;
    }
    let lengths = road.shape.windows(2).map(|p| p[0].distance(p[1]));
    let before: f64 = lengths.clone().take(segment).sum();
    let total: f64 = lengths.sum();
    if total <= 0.0 {
        return None;
    }
    let fraction = ((before + a.distance(b) * t) / total).clamp(0.0, 1.0);
    Some(Candidate { position: Position { road: id, fraction }, projected, snap_distance_m })
}
