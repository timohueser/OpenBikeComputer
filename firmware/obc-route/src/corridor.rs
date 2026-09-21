//! The detour planner's geometric blacklist: a corridor around the skipped route span.
//!
//! A detour must not re-follow the route it detours around. The route polyline is planner GPX and
//! is not graph-aligned, so the skipped span cannot be resolved to nav-graph edges by id. Instead
//! the span is downsampled into a small resident polyline, and the A* settle loop skips any
//! candidate edge whose two endpoints both lie within [`CORRIDOR_WIDTH_M`] of it.
//!
//! The both-endpoints rule is the "roughly parallel" test, without a tangent heuristic that
//! misfires at bends. A parallel side street is blocked. A bridge crossing the span keeps its
//! endpoints off to either side, and a junction edge leaving the span keeps its far endpoint
//! outside, so both stay usable.

use heapless::Vec;
use obc_map_scene::BBox;

use crate::geo::{inflated_bbox, project_to_segment};
use crate::reader::RouteReader;
use obc_map_scene::{cos_lat, ground_dist_m_cl};

/// Max resident corridor sample points. A longer span widens its sampling stride to fit, so this
/// is a hard cap by construction, never an overflow.
pub const CORRIDOR_MAX_PTS: usize = 128;

/// Along-route sampling interval floor, m. It is about half the corridor width, so the chord
/// between samples stays inside the width tolerance at any plausible bend.
pub(crate) const CORRIDOR_MIN_SAMPLE_M: f32 = 40.0;

/// Half-width of the blacklisted corridor, m.
pub(crate) const CORRIDOR_WIDTH_M: f32 = 40.0;

/// Endpoint exemption radius, m. An edge with either endpoint this close to the detour's start-
/// or goal-snap node is never blacklisted, so A* can always leave the start and reach the goal.
/// Both of those sit on or near the route line, so it must exceed
/// [`SNAP_RADIUS_M`](crate::nav::SNAP_RADIUS_M).
pub(crate) const CORRIDOR_EXEMPT_M: f32 = 300.0;

/// Below this skipped-span length the two exemption discs swallow the whole corridor and the
/// detour can re-follow the route. The chooser uses the same value as its minimum rejoin
/// distance, so such a request cannot be committed.
pub const MIN_DETOUR_SPAN_M: u32 = 600;

/// The resident corridor: the skipped span downsampled to at most [`CORRIDOR_MAX_PTS`] points,
/// its inflated bbox as a prefilter, and the two snapped detour endpoints as exemptions.
///
/// [`build`](Self::build) runs before planning starts, because it reads the route source, which
/// the planner's `step` never sees.
#[derive(Debug, Clone)]
pub struct Corridor {
    pts: Vec<(i32, i32), CORRIDOR_MAX_PTS>,
    /// Union of `pts`, pre-inflated by [`CORRIDOR_WIDTH_M`]. It is the cheap reject for edges
    /// nowhere near the span.
    bbox: BBox,
    /// Start- and goal-snap node coordinates, `None` until the planner's snaps resolve.
    exempt: Option<[(i32, i32); 2]>,
    /// `cos_lat` hoisted at the span's first point: one band for the whole corridor test.
    cl: f32,
    degenerate: bool,
}

impl Corridor {
    /// Downsample the active route's `[progress_m, target_m]` span into a corridor. It keeps a
    /// point every `max(CORRIDOR_MIN_SAMPLE_M, span/(CORRIDOR_MAX_PTS-1))` metres, plus both span
    /// endpoints.
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
                // Chunk seams repeat the boundary point, and a zero-length hop advances nothing.
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

        let bbox = inflated_bbox(pts.iter().copied(), cl, CORRIDOR_WIDTH_M);
        Corridor { pts, bbox, exempt: None, cl, degenerate }
    }

    /// Record the snapped detour endpoints once the planner's snaps resolve. Edges near either
    /// are exempt from the blacklist, so A* can leave and arrive.
    pub fn set_exempt_nodes(&mut self, start: (i32, i32), goal: (i32, i32)) {
        self.exempt = Some([start, goal]);
    }

    /// True when the span is shorter than [`MIN_DETOUR_SPAN_M`], so the exemption discs overlap
    /// the whole span and planning would re-follow the route. The chooser gates commits on this.
    pub fn is_degenerate(&self) -> bool {
        self.degenerate || self.pts.len() < 2
    }

    /// Should the candidate edge `a` to `b` be skipped? The settle loop calls this per neighbour
    /// with the two resident node coordinates. The edge polyline is never fetched, so the chord
    /// is the test geometry, which the sampling floor keeps honest at corridor scale.
    pub fn blocks(&self, a: (i32, i32), b: (i32, i32)) -> bool {
        if self.is_degenerate() {
            return false;
        }
        // Cheap reject: both endpoints outside the inflated bbox.
        if !self.bbox_contains(a) && !self.bbox_contains(b) {
            return false;
        }
        // An edge touching the take-off or landing neighbourhood stays usable.
        if let Some(ex) = self.exempt {
            for e in ex {
                if ground_dist_m_cl(a, e, self.cl) <= CORRIDOR_EXEMPT_M
                    || ground_dist_m_cl(b, e, self.cl) <= CORRIDOR_EXEMPT_M
                {
                    return false;
                }
            }
        }
        // Blocked only when both endpoints hug the span. This is the parallelism proxy.
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

    /// The kept sample count.
    pub fn len(&self) -> usize {
        self.pts.len()
    }

    /// True when no span geometry was captured at all.
    pub fn is_empty(&self) -> bool {
        self.pts.is_empty()
    }
}
