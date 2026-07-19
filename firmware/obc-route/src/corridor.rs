//! The detour planner's geometric blacklist: a corridor around the skipped route span.
//!
//! A routed detour (#882) must not simply re-follow the route it is detouring around, and the
//! route polyline is planner GPX — not guaranteed to be graph-aligned — so the skipped span
//! cannot be resolved to nav-graph edges by id. Instead the span between the rider's projection
//! and the chosen rejoin point is downsampled into a small resident polyline, and the A* settle
//! loop skips any candidate edge whose **both** endpoints lie within [`CORRIDOR_WIDTH_M`] of it.
//!
//! Both-endpoints proximity doubles as the "roughly parallel" test the issue asks for, without a
//! tangent heuristic that misfires at bends:
//! - a parallel side street inside the corridor → both endpoints near → blocked;
//! - the blocked road itself → blocked;
//! - a grade-separated bridge *crossing* the span → its endpoints sit off to either side, farther
//!   than the corridor is wide → stays usable;
//! - an at-grade junction edge leaving the span → its far endpoint is outside → stays usable, so
//!   crossing or turning off the blocked road remains possible.
//!
//! The corridor cannot know how far the real-world obstruction extends; the rider escalating the
//! rejoin distance in the chooser is the semantic backstop (#882 §3). All constants here are
//! deliberately untuned first values — revisit on-glass.

use heapless::Vec;
use obc_reader::{BBox, M_PER_DEG};

use crate::geo::{cos_lat, ground_dist_m_cl, project_to_segment};
use crate::reader::RouteReader;

/// Max resident corridor sample points (8 B each → ~1 KB). A longer span widens its sampling
/// stride to fit, so this is a hard cap by construction, never an overflow.
pub const CORRIDOR_MAX_PTS: usize = 128;

/// Along-route sampling interval floor, m — about half the corridor width, so the chord between
/// samples deviates from the true route by less than the width tolerance at any plausible bend.
pub const CORRIDOR_MIN_SAMPLE_M: f32 = 40.0;

/// Half-width of the blacklisted corridor, m (#882: "within ~30–50 m").
pub const CORRIDOR_WIDTH_M: f32 = 40.0;

/// Endpoint exemption radius, m: an edge with either endpoint within this of the detour's
/// start- or goal-snap node is never blacklisted, so A* can always leave the start and reach
/// the goal — both of which sit on or near the route line ([`SNAP_RADIUS_M`] = 250, + margin).
///
/// [`SNAP_RADIUS_M`]: crate::nav::SNAP_RADIUS_M
pub const CORRIDOR_EXEMPT_M: f32 = 300.0;

/// Below this skipped-span length the two exemption discs swallow the whole corridor and the
/// "detour" can simply re-follow the route ([`Corridor::is_degenerate`]); the chooser uses the
/// same constant as its minimum rejoin distance so such a request can't be committed.
pub const MIN_DETOUR_SPAN_M: u32 = 600;

/// The resident corridor: the skipped span downsampled to at most [`CORRIDOR_MAX_PTS`] points,
/// its inflated bbox as a prefilter, and the two snapped detour endpoints as exemptions.
///
/// Built host-side by [`build`](Self::build) before planning starts (it reads the *route*
/// source, which the planner's `step` deliberately never sees), then handed to
/// [`NavPlanner::new_detour`](crate::nav::NavPlanner::new_detour), which fills in the
/// exemption nodes once its snaps resolve.
#[derive(Debug, Clone)]
pub struct Corridor {
    pts: Vec<(i32, i32), CORRIDOR_MAX_PTS>,
    /// Union of `pts`, pre-inflated by [`CORRIDOR_WIDTH_M`] — the cheap reject for the settle
    /// loop's common case (edges nowhere near the span).
    bbox: BBox,
    /// Start-/goal-snap node coords; `None` until [`set_exempt_nodes`](Self::set_exempt_nodes).
    exempt: Option<[(i32, i32); 2]>,
    /// `cos_lat` hoisted at the span's first point — one band for the whole corridor test,
    /// consistent with how the matcher measures per-chunk.
    cl: f32,
    degenerate: bool,
}

impl Corridor {
    /// Downsample the active route's `[progress_m, target_m]` span into a corridor. Streams the
    /// span chunk-clipped (no retained route copy); keeps a point every
    /// `max(CORRIDOR_MIN_SAMPLE_M, span/(CORRIDOR_MAX_PTS-1))` meters plus both span endpoints.
    pub fn build(orig: &RouteReader, progress_m: u32, target_m: u32) -> Corridor {
        let span_m = target_m.saturating_sub(progress_m);
        let degenerate = span_m < MIN_DETOUR_SPAN_M;
        let stride = if span_m == 0 {
            CORRIDOR_MIN_SAMPLE_M
        } else {
            (span_m as f32 / (CORRIDOR_MAX_PTS - 1) as f32).max(CORRIDOR_MIN_SAMPLE_M)
        };

        let mut pts: Vec<(i32, i32), CORRIDOR_MAX_PTS> = Vec::new();
        let mut cl = 1.0f32;
        let mut since_kept = 0.0f32;
        let mut last_seen: Option<(i32, i32)> = None;
        orig.visit_points_between(progress_m, target_m, |slice| {
            for &p in slice {
                if pts.is_empty() {
                    cl = cos_lat(p.1);
                    let _ = pts.push(p);
                    last_seen = Some(p);
                    continue;
                }
                let prev = last_seen.unwrap_or(p);
                // Chunk seams repeat the boundary point; a zero-length hop advances nothing.
                since_kept += ground_dist_m_cl(prev, p, cl);
                last_seen = Some(p);
                if since_kept >= stride && !pts.is_full() {
                    let _ = pts.push(p);
                    since_kept = 0.0;
                }
            }
        });
        // Always keep the span's true end so the corridor reaches the rejoin point.
        if let Some(end) = last_seen {
            if pts.last() != Some(&end) {
                if pts.is_full() {
                    let n = pts.len();
                    pts[n - 1] = end;
                } else {
                    let _ = pts.push(end);
                }
            }
        }

        let bbox = inflated_bbox(&pts, cl);
        Corridor { pts, bbox, exempt: None, cl, degenerate }
    }

    /// Record the snapped detour endpoints (start node, goal node) once the planner's snaps
    /// resolve — edges near either are exempt from the blacklist so A* can leave and arrive.
    /// Called by the planner after its snap phases; public for the corridor's own tests.
    pub fn set_exempt_nodes(&mut self, start: (i32, i32), goal: (i32, i32)) {
        self.exempt = Some([start, goal]);
    }

    /// True when the span is too short for the corridor to bite ([`MIN_DETOUR_SPAN_M`]): the
    /// exemption discs overlap the whole span, so planning would just re-follow the route.
    /// The chooser gates commits on this.
    pub fn is_degenerate(&self) -> bool {
        self.degenerate || self.pts.len() < 2
    }

    /// Should the candidate edge `a → b` be skipped? Called per neighbor from the A* settle
    /// loop with the two node coordinates that are already resident (the real edge polyline is
    /// never fetched during settle — the chord is the test geometry, which the sampling floor
    /// keeps honest at corridor scale). Public for the corridor's own tests.
    pub fn blocks(&self, a: (i32, i32), b: (i32, i32)) -> bool {
        if self.is_degenerate() {
            return false;
        }
        // Cheap reject: both endpoints outside the inflated bbox.
        if !self.bbox_contains(a) && !self.bbox_contains(b) {
            return false;
        }
        // Exemption: an edge touching the take-off / landing neighborhoods stays usable.
        if let Some(ex) = self.exempt {
            for e in ex {
                if ground_dist_m_cl(a, e, self.cl) <= CORRIDOR_EXEMPT_M
                    || ground_dist_m_cl(b, e, self.cl) <= CORRIDOR_EXEMPT_M
                {
                    return false;
                }
            }
        }
        // Blocked only when BOTH endpoints hug the span — the parallelism proxy.
        self.near_span(a) && self.near_span(b)
    }

    /// Is `p` within [`CORRIDOR_WIDTH_M`] of the downsampled span polyline?
    fn near_span(&self, p: (i32, i32)) -> bool {
        for w in self.pts.windows(2) {
            let (_, d) = project_to_segment(w[0], w[1], p, self.cl);
            if d <= CORRIDOR_WIDTH_M {
                return true;
            }
        }
        false
    }

    fn bbox_contains(&self, p: (i32, i32)) -> bool {
        p.0 >= self.bbox.min_lon && p.0 <= self.bbox.max_lon && p.1 >= self.bbox.min_lat && p.1 <= self.bbox.max_lat
    }

    /// The kept sample count (diagnostics + tests).
    pub fn len(&self) -> usize {
        self.pts.len()
    }

    /// True when no span geometry was captured at all.
    pub fn is_empty(&self) -> bool {
        self.pts.is_empty()
    }
}

/// Union bbox of `pts`, inflated by [`CORRIDOR_WIDTH_M`] converted to microdegrees at band `cl`.
fn inflated_bbox(pts: &[(i32, i32)], cl: f32) -> BBox {
    let mut bbox = BBox { min_lon: i32::MAX, min_lat: i32::MAX, max_lon: i32::MIN, max_lat: i32::MIN };
    for &(lon, lat) in pts {
        bbox.min_lon = bbox.min_lon.min(lon);
        bbox.max_lon = bbox.max_lon.max(lon);
        bbox.min_lat = bbox.min_lat.min(lat);
        bbox.max_lat = bbox.max_lat.max(lat);
    }
    if pts.is_empty() {
        return BBox { min_lon: 0, min_lat: 0, max_lon: 0, max_lat: 0 };
    }
    let m_per_udeg_lat = M_PER_DEG as f32 * 1e-6;
    let pad_lat = (CORRIDOR_WIDTH_M / m_per_udeg_lat) as i32 + 1;
    let pad_lon = (CORRIDOR_WIDTH_M / (m_per_udeg_lat * cl.max(0.05))) as i32 + 1;
    bbox.min_lon -= pad_lon;
    bbox.max_lon += pad_lon;
    bbox.min_lat -= pad_lat;
    bbox.max_lat += pad_lat;
    bbox
}
