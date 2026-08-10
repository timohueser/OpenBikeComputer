//! Corridor construction and product selection — the frozen client policy, mirrored from the
//! iOS reference implementation so the simulator and the phone answer the same question the same
//! way.
//!
//! Two rules carry the epic's honesty contract:
//!
//! - **Containment, not overlap.** A product answers a corridor only if it covers all of it.
//! - **Expired is skipped and reported, never downgraded silently.** An expired product cannot be
//!   selected, cannot shadow a lower tier, and its identity is carried out in the report so the
//!   dev panel can say *why* the rider is on the floor product.
//!
//! Nothing here branches on a product **id**. Adding a region is a baker deploy.

use crate::manifest::{Bbox, Manifest, Product, METRES_PER_DEGREE_LAT};

/// The two-hour question the rain map answers.
pub const HORIZON_S: i64 = 2 * 3600;
/// How old an observation frame may be and still be worth fetching. Beyond this a "current"
/// frame would be a lie told with a true timestamp.
pub const MAX_OBSERVATION_AGE_S: i64 = 6 * 3600;
/// A manifest stamped this far in the future means the *local* clock is wrong. Reported, never
/// compensated: silently shifting time is how stale rain becomes a dry claim.
pub const CLOCK_SKEW_TOLERANCE_S: i64 = 15 * 60;

/// Corridor sizing (metres), mirroring `WeatherCorridor` on the phone exactly. The margin is the
/// *disc radius* around each sampled point, never added to the reach — so the ceiling on the
/// projection really is [`MAX_REACH_M`] and not `MAX_REACH_M + LATERAL_MARGIN_M`.
pub const LATERAL_MARGIN_M: f64 = 5_000.0;
/// The floor: a rider who might go any direction gets a disc, not a fabricated heading.
pub const MIN_REACH_M: f64 = 10_000.0;
/// The ceiling, so an implausible speed cannot turn into a continental corridor.
pub const MAX_REACH_M: f64 = 120_000.0;
/// How many points along the projected track get their own disc (the phone's number).
const TRACK_SAMPLES: u32 = 8;

/// What the device vouched for about the rider, exactly as §11.4's validity flags express it: a
/// field is `None` when the device did **not** vouch for it, never a sentinel. A NaN bearing is
/// not a bearing either — that distinction is load-bearing (see [`Corridor::projected`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Fix {
    pub lat_udeg: i32,
    pub lon_udeg: i32,
    /// Travel bearing, meteorological degrees.
    pub bearing_deg: Option<f64>,
    /// Ground speed, metres per second.
    pub speed_ms: Option<f64>,
    /// The route still ahead of the rider, when one is being navigated — real geometry beats any
    /// projection of it.
    pub route_ahead: Vec<(i32, i32)>,
}

/// A request window in integer microdegrees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Corridor {
    pub bounds: Bbox,
    /// The rider's own position, kept so a shrunken bundle window re-centres on the rider rather
    /// than on the midpoint of a corridor they are already leaving.
    pub lat_udeg: i32,
    pub lon_udeg: i32,
    /// True when the device vouched for neither a usable bearing nor a speed, so this is a plain
    /// disc rather than a directed corridor. Evidence for the dev panel; never control flow.
    pub undirected: bool,
}

impl Corridor {
    /// A plain disc of `radius_m` around `(lat_udeg, lon_udeg)`: the shape a rider with no
    /// trustworthy heading gets, and the simulator's `--weather-radius-km` override.
    pub fn around(lat_udeg: i32, lon_udeg: i32, radius_m: f64) -> Self {
        Self {
            bounds: Bbox::around(i64::from(lat_udeg), i64::from(lon_udeg), radius_m),
            lat_udeg,
            lon_udeg,
            undirected: true,
        }
    }

    /// The corridor for one fix — the phone's `WeatherCorridor.projected`, rule for rule.
    ///
    /// Three inputs in order of trust: the route ahead (geometry the rider intends to follow), the
    /// bearing/speed cone (what the device measured), and — when neither is vouched for — a plain
    /// disc. Every branch unions the position's own disc, so the rider's cell is inside the
    /// corridor even at a standstill.
    ///
    /// **Why this is not cosmetic:** selection is by *containment*, so a directed corridor and a
    /// disc of the same reach can land on different tiers. Answering with a disc where the phone
    /// projects a cone is exactly the parity break this mirrors away.
    pub fn projected(fix: &Fix) -> Self {
        let lat = i64::from(fix.lat_udeg);
        let lon = i64::from(fix.lon_udeg);
        let reach = fix
            .speed_ms
            .filter(|speed| speed.is_finite() && *speed >= 0.0)
            .map(|speed| (speed * HORIZON_S as f64).clamp(MIN_REACH_M, MAX_REACH_M));
        // A non-finite bearing is *not* a bearing: taking the directed branch with a NaN course
        // produces no forward reach at all and collapses the corridor below the undirected floor.
        let directed = reach.is_some() && fix.bearing_deg.is_some_and(f64::is_finite);
        let mut bounds = Bbox::around(lat, lon, if directed { LATERAL_MARGIN_M } else { reach.unwrap_or(MIN_REACH_M) });

        if let (true, Some(reach), Some(bearing)) = (directed, reach, fix.bearing_deg) {
            // Sample the track ahead rather than only its endpoint: at high latitudes the
            // straight-line box would miss the middle of a curving path.
            let radians = bearing.to_radians();
            let cos = (f64::from(fix.lat_udeg) / 1e6).to_radians().cos().max(0.05);
            for step in 1..=TRACK_SAMPLES {
                let distance = reach * f64::from(step) / f64::from(TRACK_SAMPLES);
                let north = distance * radians.cos();
                let east = distance * radians.sin();
                let point = Bbox::around(
                    lat + (north / METRES_PER_DEGREE_LAT * 1e6).round() as i64,
                    lon + (east / (METRES_PER_DEGREE_LAT * cos) * 1e6).round() as i64,
                    LATERAL_MARGIN_M,
                );
                bounds = bounds.union(&point);
            }
        }

        // Only the stretch inside the reach is added — a 300 km route must not become a 300 km
        // corridor.
        if !fix.route_ahead.is_empty() {
            let limit = reach.unwrap_or(MIN_REACH_M);
            let mut travelled = 0.0;
            let mut previous = (fix.lat_udeg, fix.lon_udeg);
            for point in &fix.route_ahead {
                travelled += haversine_m(previous, *point);
                previous = *point;
                if travelled > limit {
                    break;
                }
                bounds = bounds.union(&Bbox::around(i64::from(point.0), i64::from(point.1), LATERAL_MARGIN_M));
            }
        }

        // v1 grids never cross the antimeridian, so a corridor that would is clamped to the
        // rider's hemisphere rather than wrapped into a window meaning the far side of the planet.
        // The lost sliver reads as "not covered", which is honest.
        bounds.west_udeg = bounds.west_udeg.max(-180_000_000);
        bounds.east_udeg = bounds.east_udeg.min(180_000_000);
        Self { bounds, lat_udeg: fix.lat_udeg, lon_udeg: fix.lon_udeg, undirected: !directed }
    }

    /// The reach a rider at `speed_ms` covers inside the two-hour horizon, clamped to the sane
    /// band. `None` when the device did not vouch for a speed — the caller falls back to the
    /// undirected floor rather than inventing one.
    pub fn reach_m(speed_ms: Option<f64>) -> Option<f64> {
        speed_ms
            .filter(|speed| speed.is_finite() && *speed >= 0.0)
            .map(|speed| (speed * HORIZON_S as f64).clamp(MIN_REACH_M, MAX_REACH_M))
    }
}

/// Great-circle metres between two microdegree points — the phone's `Coordinate.distance(to:)`.
fn haversine_m(from: (i32, i32), to: (i32, i32)) -> f64 {
    const EARTH_RADIUS_M: f64 = 6_371_000.0;
    let lat1 = (f64::from(from.0) / 1e6).to_radians();
    let lat2 = (f64::from(to.0) / 1e6).to_radians();
    let d_lat = lat2 - lat1;
    let d_lon = (f64::from(to.1) / 1e6 - f64::from(from.1) / 1e6).to_radians();
    let a = (d_lat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (d_lon / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_M * a.sqrt().atan2((1.0 - a).sqrt())
}

/// Why there is no rain map. Every variant is a *stated* reason: the UI never has to guess, and
/// none of them may be rendered as "dry".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoRainMap {
    /// No product's window covers the corridor at all.
    CorridorNotCovered,
    /// Products cover it, but every one is past its staleness deadline.
    AllCoveringProductsExpired { latest_deadline: i64 },
    /// Fresh products cover it, but none has a frame inside the usable time window.
    NoFramesInWindow,
    /// The manifest itself could not be had.
    ServiceUnavailable,
    /// Every frame of the selected product failed to fetch or verify.
    FramesUnavailable,
}

impl std::fmt::Display for NoRainMap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NoRainMap::CorridorNotCovered => write!(f, "no product covers this corridor"),
            NoRainMap::AllCoveringProductsExpired { latest_deadline } => {
                write!(f, "every covering product expired (latest deadline {latest_deadline})")
            }
            NoRainMap::NoFramesInWindow => write!(f, "no covering product has a frame in the usable window"),
            NoRainMap::ServiceUnavailable => write!(f, "the weather service is unreachable"),
            NoRainMap::FramesUnavailable => write!(f, "every frame of the selected product failed"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionReport {
    /// Ids of covering products that were past their deadline. Reported, never used.
    pub expired: Vec<String>,
    /// The local clock looks wrong against the manifest's own stamp.
    pub clock_skew_suspected: bool,
}

/// Pick the highest-tier fresh product that covers the corridor and actually has frames.
///
/// Ordering is lowest tier, then newest `reference_time`, then lexicographically smallest id —
/// total and deterministic, so two runs against one manifest build the same bundle.
pub fn select<'m>(
    manifest: &'m Manifest,
    corridor: &Corridor,
    now: i64,
) -> (Result<&'m Product, NoRainMap>, SelectionReport) {
    let clock_skew_suspected = manifest.generated_at - now > CLOCK_SKEW_TOLERANCE_S;
    let covering: Vec<&Product> =
        manifest.products.iter().filter(|product| product.bounds.contains(&corridor.bounds)).collect();
    if covering.is_empty() {
        return (Err(NoRainMap::CorridorNotCovered), SelectionReport { expired: Vec::new(), clock_skew_suspected });
    }
    let expired: Vec<String> = covering.iter().filter(|p| !p.is_fresh(now)).map(|p| p.id.clone()).collect();
    let fresh: Vec<&Product> = covering.iter().copied().filter(|p| p.is_fresh(now)).collect();
    if fresh.is_empty() {
        let latest_deadline = covering.iter().map(|p| p.staleness_deadline).max().unwrap_or(now);
        return (
            Err(NoRainMap::AllCoveringProductsExpired { latest_deadline }),
            SelectionReport { expired, clock_skew_suspected },
        );
    }
    // Freshness is not answerability. A product inside its deadline whose frames all sit outside
    // the usable window must fall through to the next tier rather than shadow it with nothing.
    let Some(best) = fresh.into_iter().filter(|product| !usable_frames(product, now).is_empty()).min_by(|a, b| {
        a.tier.cmp(&b.tier).then_with(|| b.reference_time.cmp(&a.reference_time)).then_with(|| a.id.cmp(&b.id))
    }) else {
        return (Err(NoRainMap::NoFramesInWindow), SelectionReport { expired, clock_skew_suspected });
    };
    (Ok(best), SelectionReport { expired, clock_skew_suspected })
}

/// The frames of `product` worth fetching at `now`: inside the two-hour horizon ahead and no
/// older than a genuinely useful observation.
pub fn usable_frames(product: &Product, now: i64) -> Vec<&crate::manifest::Frame> {
    product
        .frames
        .iter()
        .filter(|frame| frame.valid_at <= now + HORIZON_S && frame.valid_at >= now - MAX_OBSERVATION_AGE_S)
        .collect()
}
