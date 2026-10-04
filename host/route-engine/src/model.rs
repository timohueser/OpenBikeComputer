use serde::{Deserialize, Serialize};

pub type Cost = u64;
pub const BIKE: u8 = 1;
pub const FOOT: u8 = 2;
pub const PUSH: u8 = 4;
pub const NO_ELEVATION: f32 = f32::MIN;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Point {
    pub lat: i32,
    pub lon: i32,
    pub elevation: f32,
}

impl Point {
    pub fn distance(self, other: Self) -> f64 {
        let y = (other.lat as f64 - self.lat as f64) * 0.111195;
        let x = (other.lon as f64 - self.lon as f64)
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
    pub reversed: bool,
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
    /// A bridge or tunnel uses endpoint-interpolated elevation.
    pub structure: bool,
    pub shape: Vec<Point>,
}

impl Road {
    pub fn mtb_scale(&self) -> Option<u8> {
        (self.difficulty != 255).then_some(self.difficulty)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Graph {
    pub points: Vec<Point>,
    pub node_ids: Vec<i64>,
    pub node_access: Vec<u8>,
    pub osm: crate::osm::Data,
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
    pub weighting: Weighting,
    pub walking: bool,
    pub pushing: bool,
    pub ferries: bool,
    pub max_difficulty: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Weighting {
    RoadBike(RoadBike),
    Weighted { surface: [f64; 6], road: [f64; 7], climb: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RoadBike {
    Balanced,
    Shorter,
    Smoother,
    LessClimbing,
    Quieter,
}

impl Profile {
    pub fn presets() -> Vec<Self> {
        let mut profiles = vec![
            Self {
                name: "touring".into(),
                walking: false,
                weighting: Weighting::Weighted {
                    surface: [1.4, 1.0, 1.15, 1.6, 2.5, 5.0],
                    road: [1.0, 1.15, 3.0, 1.2, 2.0, 8.0, 2.0],
                    climb: 10.0,
                },
                pushing: true,
                ferries: true,
                max_difficulty: 1,
            },
            Self {
                name: "gravel".into(),
                walking: false,
                weighting: Weighting::Weighted {
                    surface: [1.3, 1.4, 1.0, 1.0, 1.5, 3.5],
                    road: [1.1, 1.2, 4.0, 1.0, 1.7, 8.0, 2.0],
                    climb: 6.0,
                },
                pushing: true,
                ferries: true,
                max_difficulty: 1,
            },
            Self {
                name: "road".into(),
                walking: false,
                weighting: Weighting::RoadBike(RoadBike::Balanced),
                pushing: true,
                ferries: true,
                max_difficulty: 0,
            },
            Self {
                name: "hiking".into(),
                walking: true,
                weighting: Weighting::Weighted {
                    surface: [1.2, 1.3, 1.1, 1.0, 1.0, 1.5],
                    road: [1.4, 2.0, 8.0, 1.1, 1.0, 1.2, 2.0],
                    climb: 4.0,
                },
                pushing: true,
                ferries: true,
                // Up to `alpine_hiking` (T4), the route of marked alpine summits. T5 and T6 need climbing.
                max_difficulty: 4,
            },
        ];
        let mut mtb = profiles[1].clone();
        mtb.name = "mtb".into();
        mtb.weighting = Weighting::Weighted {
            surface: [1.4, 1.3, 1.1, 1.0, 1.0, 1.8],
            road: [1.2, 1.4, 4.0, 1.0, 1.0, 8.0, 2.0],
            climb: 6.0,
        };
        mtb.max_difficulty = 3;
        profiles.push(mtb);
        let variants: Vec<_> = profiles
            .iter()
            .flat_map(|profile| {
                ["shorter", "smoother", "less-climbing"].map(|variant| {
                    let mut p = profile.clone();
                    p.name = format!("{}/{variant}", profile.name);
                    match &mut p.weighting {
                        Weighting::RoadBike(road) => {
                            *road = match variant {
                                "shorter" => RoadBike::Shorter,
                                "smoother" => RoadBike::Smoother,
                                _ => RoadBike::LessClimbing,
                            }
                        }
                        Weighting::Weighted { surface, road, climb } => match variant {
                            "shorter" => {
                                *road = [1.0; 7];
                                *climb = 0.0;
                            }
                            "smoother" => {
                                for (weight, minimum) in surface.iter_mut().zip([2.0, 1.0, 1.4, 3.0, 6.0, 12.0]) {
                                    *weight = weight.max(minimum);
                                }
                            }
                            _ => *climb *= 3.0,
                        },
                    }
                    p
                })
            })
            .collect();
        profiles.extend(variants);
        let mut quieter = profiles[2].clone();
        quieter.name = "road/quieter".into();
        quieter.weighting = Weighting::RoadBike(RoadBike::Quieter);
        profiles.push(quieter);
        profiles
    }

    pub fn validate(&self) -> Result<(), String> {
        if let Weighting::Weighted { surface, road, climb } = &self.weighting {
            if surface.iter().chain(road).any(|v| !v.is_finite() || *v < 0.01 || *v > 1000.0)
                || !climb.is_finite()
                || !(0.0..=1000.0).contains(climb)
            {
                return Err("Weights must be finite, positive and at most 1000; climb may be zero".into());
            }
        }
        Ok(())
    }

    pub fn permits(&self, road: &Road) -> bool {
        let permitted = if self.walking {
            road.access & FOOT != 0
        } else {
            road.access & BIKE != 0 || self.pushing && road.access & PUSH != 0
        };
        let too_difficult = if self.walking {
            road.hiking_difficulty.is_some_and(|d| d > self.max_difficulty)
        } else {
            road.mtb_scale().is_some_and(|d| d > self.max_difficulty) || road.hiking_difficulty.is_some_and(|d| d > 2)
        };
        permitted && (road.class != 6 || self.ferries) && !too_difficult
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
        if !(1.0..=80.0).contains(&self.cycling_kmh)
            || !(0.5..=15.0).contains(&self.walking_kmh)
            || !(0.25..=4.0).contains(&self.personal_multiplier)
        {
            return Err("Pace requires cycling 1–80 km/h, walking 0.5–15 km/h and multiplier 0.25–4".into());
        }
        Ok(())
    }

    pub fn seconds(&self, road: &Road, profile: &Profile) -> f64 {
        road.shape
            .windows(2)
            .map(|p| {
                self.segment_seconds(
                    p[0],
                    p[1],
                    profile.walking || road.access & BIKE == 0,
                    profile.name.starts_with("mtb"),
                )
            })
            .sum()
    }

    pub fn segment_seconds(&self, from: Point, to: Point, walk: bool, mtb: bool) -> f64 {
        let p = [from, to];
        let distance = p[0].distance(p[1]);
        let gradient = if p[0].elevation == NO_ELEVATION || p[1].elevation == NO_ELEVATION {
            0.0
        } else {
            (p[1].elevation as f64 - p[0].elevation as f64) / distance.max(1.0)
        };
        let minutes_per_km = if walk {
            60.0 / self.walking_kmh * (3.5 * ((gradient.clamp(-0.5, 0.5) + 0.05).abs() - 0.05)).exp()
        } else {
            60.0 / self.cycling_kmh * cycling_pace(gradient, mtb) / cycling_pace(0.0, false)
        };
        distance / 1000.0 * minutes_per_km * 60.0 * self.personal_multiplier
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
    pub fn add(&mut self, road: &Road, pace: &Pace, profile: &Profile) {
        self.distance_m += road.length_m as u64;
        self.ascent_m += road.ascent_m as u64;
        self.descent_m += road.descent_m as u64;
        self.seconds += pace.seconds(road, profile);
        self.surface_m[road.surface as usize] += road.length_m as u64;
        if road.shape.iter().any(|p| p.elevation == NO_ELEVATION) {
            self.unknown_elevation_m += road.length_m as u64;
        }
        if road.uncertain_access {
            self.uncertain_access_m += road.length_m as u64;
        }
        if !profile.walking && road.access & BIKE == 0 {
            self.pushing_m += road.length_m as u64;
        }
    }
}

// Frozen log-pace curve and broad bike offsets from the ride-time model. Learning stays with the host.
fn cycling_pace(gradient: f64, mtb: bool) -> f64 {
    const GRADES: [f64; 9] = [-0.2, -0.1, -0.05, -0.02, 0.0, 0.02, 0.05, 0.1, 0.2];
    const PACE: [f64; 9] = [
        3.6678441275,
        2.4586028318,
        1.8631026063,
        2.0623855840,
        2.3229872458,
        2.7963045324,
        3.8723657434,
        5.7198863509,
        8.7498015838,
    ];
    let grade = gradient.clamp(GRADES[0], GRADES[8]);
    let right = GRADES.partition_point(|&g| g < grade).clamp(1, 8);
    let t = (grade - GRADES[right - 1]) / (GRADES[right] - GRADES[right - 1]);
    (PACE[right - 1].ln() * (1.0 - t) + PACE[right].ln() * t + if mtb { 0.2243923187 } else { 0.0268886981 }).exp()
}
