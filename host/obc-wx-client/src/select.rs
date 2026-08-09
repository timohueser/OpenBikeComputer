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

use crate::manifest::{Bbox, Manifest, Product};

/// The two-hour question the rain map answers.
pub const HORIZON_S: i64 = 2 * 3600;
/// How old an observation frame may be and still be worth fetching. Beyond this a "current"
/// frame would be a lie told with a true timestamp.
pub const MAX_OBSERVATION_AGE_S: i64 = 6 * 3600;
/// A manifest stamped this far in the future means the *local* clock is wrong. Reported, never
/// compensated: silently shifting time is how stale rain becomes a dry claim.
pub const CLOCK_SKEW_TOLERANCE_S: i64 = 15 * 60;

/// Corridor sizing (metres). The simulator's rider has no route ahead of it in v1, so the
/// corridor is a disc around the fix grown by the two-hour reach of its speed — the same shape
/// the phone falls back to when a ride has no route.
pub const LATERAL_MARGIN_M: f64 = 5_000.0;
pub const MIN_RADIUS_M: f64 = 10_000.0;
pub const MAX_RADIUS_M: f64 = 120_000.0;

const METRES_PER_DEGREE_LAT: f64 = 111_320.0;

/// A request window in integer microdegrees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Corridor {
    pub bounds: Bbox,
    /// The rider's own position, kept so a shrunken bundle window re-centres on the rider rather
    /// than on the midpoint of a corridor they are already leaving.
    pub lat_udeg: i32,
    pub lon_udeg: i32,
}

impl Corridor {
    /// A disc of `radius_m` around `(lat_udeg, lon_udeg)`, clamped to the coordinate range.
    /// Longitude grows by `1 / max(0.05, cos(lat))` so a corridor stays as wide in metres near
    /// the poles; the antimeridian is clamped, never wrapped (OBCG v1 has no wrapping window).
    pub fn around(lat_udeg: i32, lon_udeg: i32, radius_m: f64) -> Self {
        let lat_deg = f64::from(lat_udeg) / 1e6;
        let lat_span = (radius_m / METRES_PER_DEGREE_LAT * 1e6).ceil() as i64;
        let cos = lat_deg.to_radians().cos().abs().max(0.05);
        let lon_span = (radius_m / (METRES_PER_DEGREE_LAT * cos) * 1e6).ceil() as i64;
        let bounds = Bbox {
            south_udeg: (i64::from(lat_udeg) - lat_span).clamp(-90_000_000, 90_000_000),
            north_udeg: (i64::from(lat_udeg) + lat_span).clamp(-90_000_000, 90_000_000),
            west_udeg: (i64::from(lon_udeg) - lon_span).clamp(-180_000_000, 180_000_000),
            east_udeg: (i64::from(lon_udeg) + lon_span).clamp(-180_000_000, 180_000_000),
        };
        Self { bounds, lat_udeg, lon_udeg }
    }

    /// The reach a rider at `speed_ms` covers in the two-hour horizon, clamped to the sane band.
    pub fn reach_m(speed_ms: Option<f64>) -> f64 {
        match speed_ms {
            Some(speed) if speed.is_finite() && speed >= 0.0 => {
                (speed * HORIZON_S as f64).clamp(MIN_RADIUS_M, MAX_RADIUS_M) + LATERAL_MARGIN_M
            }
            _ => MIN_RADIUS_M + LATERAL_MARGIN_M,
        }
    }
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
pub fn usable_frames<'p>(product: &'p Product, now: i64) -> Vec<&'p crate::manifest::Frame> {
    product
        .frames
        .iter()
        .filter(|frame| frame.valid_at <= now + HORIZON_S && frame.valid_at >= now - MAX_OBSERVATION_AGE_S)
        .collect()
}
