use serde::{Deserialize, Serialize};

pub type Cost = u64;
pub const BIKE: u8 = 1;
pub const FOOT: u8 = 2;
pub const PUSH: u8 = 4;
pub const NO_ELEVATION: i16 = i16::MIN;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Point {
    pub lat: i32,
    pub lon: i32,
    pub elevation: i16,
}

impl Point {
    pub fn distance(self, other: Self) -> f64 {
        let y = (other.lat - self.lat) as f64 * 0.111195;
        let x = (other.lon - self.lon) as f64
            * 0.111195
            * ((self.lat as f64 + other.lat as f64) * 0.5e-6).to_radians().cos();
        x.hypot(y)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Surface {
    #[default]
    Unknown,
    Paved,
    Compacted,
    Gravel,
    Dirt,
    Rough,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Road {
    pub from: u32,
    pub to: u32,
    pub way: i64,
    pub length_m: u32,
    pub ascent_m: u32,
    pub descent_m: u32,
    pub surface: Surface,
    /// 0: cycleway, 1: quiet road, 2: main road, 3: track, 4: path, 5: steps, 6: ferry.
    pub class: u8,
    pub access: u8,
    /// MTB scale; 255 means that the source has no classification.
    pub difficulty: u8,
    pub hiking_difficulty: Option<u8>,
    pub uncertain_access: bool,
    pub shape: Vec<Point>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Graph {
    pub points: Vec<Point>,
    pub roads: Vec<Road>,
    /// Forbidden transitions between directed roads. Sorted for binary search.
    pub forbidden: Vec<(u32, u32)>,
    pub forbidden_foot: Vec<(u32, u32)>,
    pub warnings: Vec<String>,
}

impl Graph {
    pub fn departures(&self) -> Vec<Vec<u32>> {
        let mut result = vec![Vec::new(); self.points.len()];
        for (id, road) in self.roads.iter().enumerate() {
            result[road.from as usize].push(id as u32);
        }
        result
    }

    pub fn permits_turn(&self, from: u32, to: u32, walking: bool) -> bool {
        let forbidden = if walking { &self.forbidden_foot } else { &self.forbidden };
        forbidden.binary_search(&(from, to)).is_err()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub walking: bool,
    pub surface_weights: [f64; 6],
    pub road_weights: [f64; 7],
    pub climb_weight: f64,
    pub pushing: bool,
    pub ferries: bool,
    pub max_difficulty: u8,
}

impl Profile {
    pub fn presets() -> Vec<Self> {
        vec![
            Self {
                name: "touring".into(),
                walking: false,
                surface_weights: [1.4, 1.0, 1.15, 1.6, 2.5, 5.0],
                road_weights: [1.0, 1.15, 3.0, 1.2, 2.0, 8.0, 2.0],
                climb_weight: 10.0,
                pushing: true,
                ferries: true,
                max_difficulty: 1,
            },
            Self {
                name: "gravel".into(),
                walking: false,
                surface_weights: [1.3, 1.4, 1.0, 1.0, 1.5, 3.5],
                road_weights: [1.1, 1.2, 4.0, 1.0, 1.7, 8.0, 2.0],
                climb_weight: 6.0,
                pushing: true,
                ferries: true,
                max_difficulty: 1,
            },
            Self {
                name: "road".into(),
                walking: false,
                surface_weights: [2.0, 1.0, 2.5, 5.0, 8.0, 15.0],
                road_weights: [1.0, 1.1, 2.5, 3.0, 6.0, 20.0, 2.0],
                climb_weight: 5.0,
                pushing: false,
                ferries: true,
                max_difficulty: 0,
            },
            Self {
                name: "hiking".into(),
                walking: true,
                surface_weights: [1.2, 1.3, 1.1, 1.0, 1.0, 1.5],
                road_weights: [1.4, 2.0, 8.0, 1.1, 1.0, 1.2, 2.0],
                climb_weight: 4.0,
                pushing: true,
                ferries: true,
                max_difficulty: 2,
            },
        ]
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.surface_weights.iter().chain(&self.road_weights).any(|v| !v.is_finite() || *v < 0.01 || *v > 1000.0)
            || !self.climb_weight.is_finite()
            || !(0.0..=1000.0).contains(&self.climb_weight)
        {
            return Err("Weights must be finite, positive and at most 1000; climb may be zero".into());
        }
        Ok(())
    }

    pub fn cost(&self, road: &Road) -> Option<Cost> {
        let permitted = if self.walking {
            road.access & FOOT != 0
        } else {
            road.access & BIKE != 0 || self.pushing && road.access & PUSH != 0
        };
        let too_difficult = if self.walking {
            road.hiking_difficulty.is_some_and(|d| d > self.max_difficulty)
        } else {
            road.difficulty != 255 && road.difficulty > self.max_difficulty
                || road.hiking_difficulty.is_some_and(|d| d > 2)
        };
        if !permitted || road.class == 6 && !self.ferries || too_difficult {
            return None;
        }
        let pushing = !self.walking && road.access & BIKE == 0;
        let cost = road.length_m as f64
            * self.surface_weights[road.surface as usize]
            * self.road_weights.get(road.class as usize)?
            * if pushing { 4.0 } else { 1.0 }
            + road.ascent_m as f64 * self.climb_weight;
        Some(cost.round().max(1.0) as Cost)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pace {
    pub cycling_kmh: f64,
    pub walking_kmh: f64,
    /// A larger value means more time. This never enters the route cost.
    pub personal_multiplier: f64,
}

impl Default for Pace {
    fn default() -> Self {
        Self { cycling_kmh: 19.0, walking_kmh: 4.5, personal_multiplier: 1.0 }
    }
}

impl Pace {
    pub fn validate(&self) -> Result<(), String> {
        if [self.cycling_kmh, self.walking_kmh, self.personal_multiplier].iter().any(|n| !n.is_finite() || *n <= 0.0) {
            return Err("Pace inputs must be finite and positive".into());
        }
        Ok(())
    }

    pub fn seconds(&self, road: &Road, walking: bool) -> f64 {
        let walk = walking || road.access & BIKE == 0;
        road.shape
            .windows(2)
            .map(|p| {
                let distance = p[0].distance(p[1]);
                let gradient = if p[0].elevation == NO_ELEVATION || p[1].elevation == NO_ELEVATION {
                    0.0
                } else {
                    (p[1].elevation as f64 - p[0].elevation as f64) / distance.max(1.0)
                };
                // Demonstration curves; the pace learner is a separate consumer of this interface.
                let speed = if walk {
                    self.walking_kmh * (-3.5 * ((gradient + 0.05).abs() - 0.05)).exp()
                } else {
                    let uphill = (1.0 + gradient.max(0.0) * 35.0).recip();
                    let downhill = 1.0 + (-gradient).clamp(0.0, 0.08) * 12.0;
                    (self.cycling_kmh * uphill * downhill).clamp(3.0, 55.0)
                };
                distance * 3.6 / speed.max(0.5) * self.personal_multiplier
            })
            .sum()
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Totals {
    pub distance_m: u64,
    pub ascent_m: u64,
    pub descent_m: u64,
    pub seconds: f64,
    pub surface_m: [u64; 6],
    pub unknown_elevation_m: u64,
    pub uncertain_access_m: u64,
    pub pushing_m: u64,
}

impl Totals {
    pub fn add(&mut self, road: &Road, pace: &Pace, walking: bool) {
        self.distance_m += road.length_m as u64;
        self.ascent_m += road.ascent_m as u64;
        self.descent_m += road.descent_m as u64;
        self.seconds += pace.seconds(road, walking);
        self.surface_m[road.surface as usize] += road.length_m as u64;
        if road.shape.iter().any(|p| p.elevation == NO_ELEVATION) {
            self.unknown_elevation_m += road.length_m as u64;
        }
        if road.uncertain_access {
            self.uncertain_access_m += road.length_m as u64;
        }
        if !walking && road.access & BIKE == 0 {
            self.pushing_m += road.length_m as u64;
        }
    }
}
