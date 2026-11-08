//! The census gate of a bake: the heights of the ways on the ground and of the rivers and streams
//! against the terrain that the bake reads, which TrailView draws.

use serde_json::{json, Value};

use crate::{Part, Terrain};

/// Chosen by the owner: a way or water this far off the terrain shows as a wall or a gorge that is
/// not there.
pub const GATE_M: f64 = 10.0;
/// The census counts the offsets in bins of BIN_M up to BINS × BIN_M; the last bin holds all past it.
const BIN_M: f64 = 0.05;
const BINS: usize = 2000;
/// The places that the census names: the worst of each side, one per square of about 100 m.
const WORST: usize = 10;
const PLACE_DEG: f64 = 0.001;

/// The offsets of a set of vertices from the terrain, height less terrain.
pub struct Census {
    vertices: u64,
    bins: Vec<u64>,
    over: u64,
    under: u64,
    /// (offset, latitude and longitude, kind), the largest above and below.
    highest: Vec<(f64, [f64; 2], String)>,
    lowest: Vec<(f64, [f64; 2], String)>,
}

impl Default for Census {
    fn default() -> Self {
        Census { vertices: 0, bins: vec![0; BINS + 1], over: 0, under: 0, highest: Vec::new(), lowest: Vec::new() }
    }
}

impl Census {
    /// Counts the vertices of `parts` with a height and a known terrain, but those that `skip` marks.
    pub fn add(&mut self, kind: &str, parts: &[Part], terrain: &dyn Terrain, skip: &dyn Fn([f64; 2]) -> bool) {
        for part in parts {
            let Some(z) = &part.z else { continue };
            for ((&point, &z), ground) in part.points.iter().zip(z).zip(terrain.heights(&part.points)) {
                let dz = z - ground;
                if !dz.is_finite() || skip(point) {
                    continue;
                }
                self.vertices += 1;
                self.bins[((dz.abs() / BIN_M) as usize).min(BINS)] += 1;
                self.over += u64::from(dz > GATE_M);
                self.under += u64::from(dz < -GATE_M);
                let at = crate::degrees(point);
                keep(&mut self.highest, (dz, at, kind), |a, b| a > b);
                keep(&mut self.lowest, (dz, at, kind), |a, b| a < b);
            }
        }
    }

    pub fn over(&self) -> u64 {
        self.over
    }

    pub fn under(&self) -> u64 {
        self.under
    }

    /// The worst places past the gate above (`high`) or below the terrain, as text.
    pub fn places(&self, high: bool) -> String {
        let list = if high { &self.highest } else { &self.lowest };
        let past = list.iter().filter(|(dz, _, _)| if high { *dz > GATE_M } else { *dz < -GATE_M });
        past.map(|(dz, [lat, lon], kind)| format!("{lat:.5},{lon:.5} {kind} {dz:+.1} m")).collect::<Vec<_>>().join("; ")
    }

    pub fn metrics(&self) -> Value {
        let share = |past: u64| past as f64 / self.vertices.max(1) as f64;
        let percentile = |p: f64| {
            let wanted = (p / 100.0 * self.vertices as f64).ceil() as u64;
            let mut seen = 0;
            let bin = self.bins.iter().position(|&n| {
                seen += n;
                seen >= wanted.max(1)
            });
            (bin.unwrap_or(BINS) as f64 + 1.0) * BIN_M
        };
        let over_5 = self.bins[(5.0 / BIN_M) as usize..].iter().sum::<u64>();
        json!({"vertices": self.vertices, "over_m": self.over, "under_m": self.under,
               "p50_m": percentile(50.0), "p90_m": percentile(90.0), "p99_m": percentile(99.0),
               "share_past_5m": (share(over_5) * 1e5).round() / 1e5,
               "highest": self.places(true), "lowest": self.places(false)})
    }
}

/// Keeps the WORST entries of `list` that come first by `before`, one per place of about PLACE_DEG.
fn keep(list: &mut Vec<(f64, [f64; 2], String)>, (dz, at, kind): (f64, [f64; 2], &str), before: fn(f64, f64) -> bool) {
    let same = |p: [f64; 2]| (p[0] - at[0]).abs() < PLACE_DEG && (p[1] - at[1]).abs() < PLACE_DEG;
    if let Some(k) = list.iter().position(|e| same(e.1)) {
        if !before(dz, list[k].0) {
            return;
        }
        list.remove(k);
    }
    if list.len() == WORST && !before(dz, list[WORST - 1].0) {
        return;
    }
    let at_index = list.iter().position(|e| before(dz, e.0)).unwrap_or(list.len());
    list.insert(at_index, (dz, at, kind.to_string()));
    list.truncate(WORST);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_census_counts_the_vertices_past_the_gate_on_each_side_and_names_the_worst() {
        let flat = |_: f64, _: f64| 500.0;
        let line = |z: Vec<f64>| Part {
            outer: false,
            points: (0..z.len()).map(|i| [0.53 + i as f64 * 1e-5, 0.36]).collect(),
            z: Some(z),
            plan: None,
        };
        let mut census = Census::default();
        census.add("road", &[line(vec![500.5, 512.0, 499.0, f64::NAN, 470.0, 520.0])], &flat, &|p| p[0] > 0.53004);
        assert_eq!((census.vertices, census.over(), census.under()), (4, 1, 1));
        assert!(census.places(true).ends_with("road +12.0 m"), "{}", census.places(true));
        assert!(census.places(false).ends_with("road -30.0 m"), "{}", census.places(false));
        assert_eq!(census.metrics()["p50_m"], 1.05);
    }
}
