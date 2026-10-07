use crate::model::{Point, Profile, Road, RoadBike, Weighting, NO_ELEVATION};
use serde::{Deserialize, Serialize};

/// Distance cost plus cumulative terrain and bend penalties along a directed road.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RoadCost {
    pub distance: f64,
    /// Geometric length fraction and accumulated penalty. Equal fractions encode a step.
    pub penalties: Vec<(f64, f64)>,
}

impl RoadCost {
    pub fn total(&self) -> u64 {
        (self.distance + self.penalties.last().map_or(0.0, |p| p.1)).round().max(1.0) as u64
    }

    /// Rounded prefixes telescope, including when a road is split at several shaping points.
    pub fn prefix(&self, fraction: f64) -> Result<u64, String> {
        if !fraction.is_finite() || !(0.0..=1.0).contains(&fraction) {
            return Err("Invalid road offset".into());
        }
        if fraction == 0.0 {
            return Ok(0);
        }
        let i = self.penalties.partition_point(|p| p.0 <= fraction);
        let a = if i == 0 { (0.0, 0.0) } else { self.penalties[i - 1] };
        let penalty =
            if let Some(b) = self.penalties.get(i) { a.1 + (b.1 - a.1) * (fraction - a.0) / (b.0 - a.0) } else { a.1 };
        let total = self.distance + self.penalties.last().map_or(0.0, |p| p.1);
        Ok((self.total() as f64 * (self.distance * fraction + penalty) / total).round() as u64)
    }

    pub fn valid(&self) -> bool {
        self.distance.is_finite()
            && self.distance > 0.0
            && self.total() < u32::MAX as u64
            && self
                .penalties
                .iter()
                .all(|&(f, c)| f.is_finite() && (0.0..=1.0).contains(&f) && c.is_finite() && c >= 0.0)
            && self.penalties.windows(2).all(|p| p[0].0 <= p[1].0 && p[0].1 <= p[1].1)
            && self.penalties.last().is_none_or(|p| p.0 == 1.0)
    }
}

/// Exact source factors shared by roads and profiles. Multiplication precedes curve compilation.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CostBasis {
    pub factor: f64,
    pub turn: f64,
    pub ferry: bool,
}

impl CostBasis {
    pub fn valid(&self) -> bool {
        self.factor.is_finite()
            && self.factor > 0.0
            && self.turn.is_finite()
            && self.turn >= 0.0
            && self.turn < u32::MAX as f64
    }

    /// The one compilation of a road's cost curve: preparation stores its total, queries recompute
    /// the curve from the same geometry, so the two always agree. Terrain and bends come from the
    /// shared geometry.
    pub fn compile(self, road: &Road, profile: &Profile) -> Result<RoadCost, String> {
        let distance = road.length_m as f64 * self.factor;
        if !(self.valid() && distance > 0.0 && distance < u32::MAX as f64) || road.shape.len() < 2 {
            return Err("Invalid road cost inputs".into());
        }
        let lengths: Vec<_> = road.shape.windows(2).map(|p| p[0].distance(p[1])).collect();
        let total: f64 = lengths.iter().sum();
        if total <= 0.0 {
            return Err("Road has zero geometric length".into());
        }
        let (up, down, cutoff) = match profile.weighting {
            Weighting::RoadBike(RoadBike::Shorter) => (0.0, 0.0, 0.015),
            Weighting::RoadBike(RoadBike::LessClimbing) => (60.0, 60.0, 0.015),
            Weighting::RoadBike(_) => (0.0, 60.0, 0.015),
            Weighting::Weighted { climb, .. } => (climb, 0.0, 0.0),
        };
        let mut result = RoadCost { distance, penalties: Vec::new() };
        let mut distance = 0.0;
        let mut penalty = 0.0;
        for (i, (&length, pair)) in lengths.iter().zip(road.shape.windows(2)).enumerate() {
            if i > 0 && self.turn != 0.0 {
                let bend = turn(road.shape[i - 1], road.shape[i], road.shape[i + 1], self.turn);
                if bend > 0.0 {
                    penalty += bend;
                    knot(&mut result.penalties, distance / total, penalty);
                }
            }
            if pair.iter().all(|p| p.elevation != NO_ELEVATION) && !self.ferry {
                let delta = pair[1].elevation as f64 - pair[0].elevation as f64;
                penalty += (delta - length * cutoff).max(0.0) * up + (-delta - length * cutoff).max(0.0) * down;
            }
            distance += length;
            knot(&mut result.penalties, (distance / total).min(1.0), penalty);
        }
        if penalty == 0.0 {
            result.penalties.clear();
        }
        if !result.valid() {
            return Err("Invalid prepared road cost".into());
        }
        Ok(result)
    }
}

fn knot(points: &mut Vec<(f64, f64)>, fraction: f64, cost: f64) {
    let next = (fraction, cost);
    if let Some(&last) = points.last() {
        let previous = if points.len() > 1 { points[points.len() - 2] } else { (0.0, 0.0) };
        // Keep changes of slope and steps, but omit collinear terrain samples.
        if fraction > last.0
            && last.0 > previous.0
            && ((next.1 - last.1) * (last.0 - previous.0) - (last.1 - previous.1) * (next.0 - last.0)).abs() < 1e-9
        {
            points.pop();
        }
    }
    points.push(next);
}

pub fn turn(a: Point, b: Point, c: Point, base: f64) -> f64 {
    let scale = (b.lat as f64 * 1e-6).to_radians().cos();
    let ab = ((b.lon as f64 - a.lon as f64) * scale, b.lat as f64 - a.lat as f64);
    let bc = ((c.lon as f64 - b.lon as f64) * scale, c.lat as f64 - b.lat as f64);
    let lengths = ab.0.hypot(ab.1) * bc.0.hypot(bc.1);
    if lengths == 0.0 {
        return 0.0;
    }
    let cosine = ((ab.0 * bc.0 + ab.1 * bc.1) / lengths).clamp(-1.0, 1.0);
    ((1.0 - cosine) * base + 0.2).floor()
}
