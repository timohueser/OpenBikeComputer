use serde::{Deserialize, Serialize};

/// Distance cost plus cumulative terrain and bend penalties along a directed road.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
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
