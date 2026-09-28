use crate::{
    model::{Point, Profile, Road, BIKE, NO_ELEVATION},
    package::{cell_key, Package, Source, CELL},
    Error, Result,
};
use serde::Serialize;
use std::collections::BTreeSet;

impl Default for Policy {
    fn default() -> Self {
        Self { radius_m: 250.0, ambiguity_m: 3.0, max_candidates: 8 }
    }
}

impl<S: Source> Package<S> {
    pub fn snap(&mut self, point: Point, metric: &str, policy: Policy) -> Result<Candidates> {
        if !policy.ambiguity_m.is_finite()
            || !(0.0..=50.0).contains(&policy.ambiguity_m)
            || policy.max_candidates == 0
            || policy.max_candidates > 16
        {
            return Err(Error::InvalidRequest("Invalid snapping policy".into()));
        }
        let profile = self.metric(metric)?.profile.clone();
        let mut roads = BTreeSet::<u32>::new();
        for cell in cells(point, policy.radius_m).map_err(Error::InvalidRequest)? {
            if let Some(key) = self.manifest.spatial.get(&cell_key(cell)) {
                roads.extend(self.read::<Vec<u32>>(key)?);
                if roads.len() > 100_000 {
                    return Err(Error::Limit);
                }
            }
        }
        let mut found = Vec::new();
        for id in roads {
            let road = self.road(id)?;
            if profile.cost(&road).is_none() {
                continue;
            }
            if let Some(candidate) = project(id, &road, point) {
                if candidate.snap_distance_m <= policy.radius_m {
                    found.push(candidate);
                }
            }
        }
        found.sort_by(|a, b| {
            a.snap_distance_m.total_cmp(&b.snap_distance_m).then(a.position.road.cmp(&b.position.road))
        });
        let eligible_roads_in_radius = found.len();
        let nearest_distance_m = found.first().map(|c| c.snap_distance_m);
        if let Some(distance) = nearest_distance_m {
            found.retain(|c| c.snap_distance_m <= distance + policy.ambiguity_m);
        }
        let candidates_in_ambiguity_band = found.len();
        let truncated = found.len() > policy.max_candidates;
        found.truncate(policy.max_candidates);
        Ok(Candidates {
            policy,
            nearest_distance_m,
            eligible_roads_in_radius,
            candidates_in_ambiguity_band,
            truncated,
            retained: found,
        })
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Position {
    pub road: u32,
    /// Fraction of the directed polyline's geometric length, from zero to one.
    pub fraction: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Candidate {
    pub position: Position,
    pub projected: Point,
    pub snap_distance_m: f64,
    pub segment: usize,
    pub segment_fraction: f64,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Policy {
    pub radius_m: f64,
    pub ambiguity_m: f64,
    pub max_candidates: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct Candidates {
    pub policy: Policy,
    pub nearest_distance_m: Option<f64>,
    pub eligible_roads_in_radius: usize,
    pub candidates_in_ambiguity_band: usize,
    pub truncated: bool,
    pub retained: Vec<Candidate>,
}

fn cells(point: Point, radius: f64) -> std::result::Result<BTreeSet<(i32, i32)>, String> {
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

pub fn project(id: u32, road: &Road, point: Point) -> Option<Candidate> {
    let lengths: Vec<_> = road.shape.windows(2).map(|p| p[0].distance(p[1])).collect();
    let total: f64 = lengths.iter().sum();
    if total <= 0.0 {
        return None;
    }
    let scale = (point.lat as f64 * 1e-6).to_radians().cos();
    let mut best: Option<Candidate> = None;
    let mut before = 0.0;
    for (segment, pair) in road.shape.windows(2).enumerate() {
        let ax = (pair[0].lon as f64 - point.lon as f64) * scale;
        let ay = pair[0].lat as f64 - point.lat as f64;
        let dx = (pair[1].lon as f64 - pair[0].lon as f64) * scale;
        let dy = pair[1].lat as f64 - pair[0].lat as f64;
        let denominator = dx * dx + dy * dy;
        let t = if denominator > 0.0 { (-(ax * dx + ay * dy) / denominator).clamp(0.0, 1.0) } else { 0.0 };
        let projected = Point {
            lat: (pair[0].lat as f64 + (pair[1].lat as f64 - pair[0].lat as f64) * t).round() as i32,
            lon: (pair[0].lon as f64 + (pair[1].lon as f64 - pair[0].lon as f64) * t).round() as i32,
            elevation: if pair.iter().any(|p| p.elevation == NO_ELEVATION) {
                NO_ELEVATION
            } else {
                (pair[0].elevation as f64 + (pair[1].elevation as f64 - pair[0].elevation as f64) * t).round() as i16
            },
        };
        let candidate = Candidate {
            position: Position { road: id, fraction: ((before + lengths[segment] * t) / total).clamp(0.0, 1.0) },
            projected,
            snap_distance_m: point.distance(projected),
            segment,
            segment_fraction: t,
        };
        if best.as_ref().is_none_or(|b| candidate.snap_distance_m < b.snap_distance_m) {
            best = Some(candidate);
        }
        before += lengths[segment];
    }
    best
}

/// Cumulative measure preserves the full prepared integer cost. Rounded prefix differences telescope.
pub fn prefix_cost(road: &Road, profile: &Profile, fraction: f64) -> std::result::Result<u64, String> {
    profile.validate()?;
    if !fraction.is_finite() || !(0.0..=1.0).contains(&fraction) {
        return Err("Invalid road offset".into());
    }
    let full = profile.cost(road).ok_or("Attachment road is excluded by the profile")?;
    if fraction == 0.0 {
        return Ok(0);
    }
    if fraction == 1.0 {
        return Ok(full);
    }
    let total: f64 = road.shape.windows(2).map(|p| p[0].distance(p[1])).sum();
    if total <= 0.0 {
        return Err("Cannot split a road without geometric length".into());
    }
    let mut distance = 0.0;
    let mut ascent = 0.0;
    let mut partial_ascent = 0.0;
    for pair in road.shape.windows(2) {
        let length = pair[0].distance(pair[1]);
        let up = if pair.iter().any(|p| p.elevation == NO_ELEVATION) {
            0.0
        } else {
            (pair[1].elevation as f64 - pair[0].elevation as f64).max(0.0)
        };
        let used = if length > 0.0 { ((total * fraction - distance) / length).clamp(0.0, 1.0) } else { 0.0 };
        partial_ascent += up * used;
        ascent += up;
        distance += length;
    }
    if ascent == 0.0 && road.ascent_m != 0 && profile.climb_weight != 0.0 {
        return Err("Cannot localize the stored climb cost without elevation samples".into());
    }
    let distance_cost = road.length_m as f64
        * profile.surface_weights[road.surface as usize]
        * profile.road_weights[road.class as usize]
        * if !profile.walking && road.access & BIKE == 0 { 4.0 } else { 1.0 };
    let climb_cost = road.ascent_m as f64 * profile.climb_weight;
    let partial = distance_cost * fraction + if ascent > 0.0 { climb_cost * partial_ascent / ascent } else { 0.0 };
    Ok((full as f64 * partial / (distance_cost + climb_cost)).round().clamp(0.0, full as f64) as u64)
}
