//! The on-screen breadcrumb — a bounded, two-tier record of where the rider has been.
//!
//! The durable ride log lives on the SD card ([`obc_route::track`]); this is the picture, held in
//! RAM so the map can draw the travelled path without re-reading storage. The recent tier is a
//! full-resolution sliding tail of the last ~2 km, a fixed ring that is never decimated. The spine
//! is the rest of the ride, held to a fixed point budget by Visvalingam–Whyatt: when it is full,
//! drop the vertex with the smallest [effective area](obc_route::tri_area_m2_cl), whose removal
//! bends the line least.
//!
//! A distance tolerance would not do. A global tolerance on a growing track sticks once any section
//! forces it up, then draws later gently-curving stretches as one chord while stale early detail
//! survives. Visvalingam always keeps exactly the budget and drops the globally least useful point,
//! so the budget redistributes to wherever the shape is.
//!
//! The tiers are disjoint: a point lives in the recent ring until it ages out, and only then is it
//! handed to the spine, so the whole trail draws as one chained polyline
//! ([`points`](Breadcrumb::points)). Both are fixed-capacity, so the renderer's polyline scratch can
//! never overrun.

use heapless::{Deque, Vec};

use crate::placement::define_placement_constructors;
use obc_map_scene::{cos_lat, ground_dist_m};
use obc_route::tri_area_m2_cl;

/// A 2-D point in microdegrees `(lon, lat)` — what the renderer projects.
type P = (i32, i32);

/// Full-resolution recent-tail capacity. At ≥[`RECENT_MIN_M`] spacing this covers the last
/// ~2 km of trail — more than the riding-zoom view ever shows.
const RECENT_CAP: usize = 256;
/// Minimum spacing (m) between recent-tail points — drops near-duplicate fixes (and a
/// stationary rider) so the ring spans real distance, not GPS jitter.
const RECENT_MIN_M: f32 = 4.0;

/// Whole-ride spine capacity (points). The spine holds exactly this many once warmed, whatever the
/// ride length, so this is the one lever for long-ride fidelity, at linear RAM cost.
const SPINE_CAP: usize = 1024;

/// The travelled path drawn on the map: a full-resolution recent tail over a coarse whole-ride
/// spine. Fed one accepted fix at a time, and cleared when a tracking session restarts.
pub struct Breadcrumb {
    recent: Deque<P, RECENT_CAP>,
    spine: Vec<P, SPINE_CAP>,
    last_recent: Option<P>,
}

impl Default for Breadcrumb {
    fn default() -> Self {
        Self::new()
    }
}

impl Breadcrumb {
    define_placement_constructors!(
        /// An empty trail.
        pub fn new();
        /// Initialize `slot` in place to the empty trail. The spine alone is 8 KB, so a by-value
        /// `Breadcrumb::new()` written through its owner costs a zeroed stack temporary plus a
        /// copy; writing the two containers straight into the slot costs their two lengths.
        pub unsafe fn init_in_place;
        fields {
            recent: Deque::new(),
            spine: Vec::new(),
            last_recent: None,
        }
    );

    /// Forget the whole trail, when a tracking session begins. A route swap alone keeps it.
    pub fn clear(&mut self) {
        self.recent.clear();
        self.spine.clear();
        self.last_recent = None;
    }

    /// Whether the trail has anything to draw yet.
    pub fn is_empty(&self) -> bool {
        self.recent.is_empty() && self.spine.is_empty()
    }

    /// Add one accepted fix `(lon, lat)` to the trail. It enters the `recent` ring, and whatever
    /// ages out of the ring is handed to the coarse `spine`.
    pub fn push(&mut self, lon: i32, lat: i32) {
        let p = (lon, lat);
        if self.last_recent.is_none_or(|q| ground_dist_m(p, q) >= RECENT_MIN_M) {
            if self.recent.is_full() {
                if let Some(aged) = self.recent.pop_front() {
                    self.spine_push(aged);
                }
            }
            let _ = self.recent.push_back(p);
            self.last_recent = Some(p);
        }
    }

    /// Append one aged-out point to the whole-ride spine, holding it to [`SPINE_CAP`] by
    /// Visvalingam–Whyatt: once full, drop the least-significant interior vertex (smallest
    /// [`tri_area_m2_cl`]) and append the new one.
    ///
    /// Index 0, the ride start, and the newest point are always kept, so the drawn line spans the
    /// whole ride and joins cleanly to `recent`.
    fn spine_push(&mut self, c: P) {
        if !self.spine.is_full() {
            let _ = self.spine.push(c);
            return;
        }
        let n = self.spine.len();
        if n < 2 {
            return; // degenerate budget (<2): keep the start, drop the rest
        }
        // `cos_lat` barely varies across one ride, so hoist it once for the whole scan. The
        // current last vertex uses the incoming `c` as its right neighbour.
        let cl = cos_lat(c.1);
        let mut min_i = 1;
        let mut min_area = f32::INFINITY;
        for i in 1..n {
            let left = self.spine[i - 1];
            let right = if i + 1 < n { self.spine[i + 1] } else { c };
            let area = tri_area_m2_cl(left, self.spine[i], right, cl);
            if area < min_area {
                min_area = area;
                min_i = i;
            }
        }
        // Drop `min_i` and append `c`: shift the tail left into the freed slot, then reuse the
        // last slot for the new point.
        for j in min_i..n - 1 {
            self.spine[j] = self.spine[j + 1];
        }
        self.spine[n - 1] = c;
    }

    /// The whole travelled path as one polyline, oldest first: the coarse spine chained to the
    /// full-resolution recent tail.
    pub fn points(&self) -> impl Iterator<Item = P> + '_ {
        self.spine.iter().copied().chain(self.recent.iter().copied())
    }

    /// Whole-ride spine points, oldest first — for introspection and tests.
    pub fn spine_iter(&self) -> impl Iterator<Item = P> + '_ {
        self.spine.iter().copied()
    }

    /// Recent-tail points, oldest first — for introspection and tests.
    pub fn recent_iter(&self) -> impl Iterator<Item = P> + '_ {
        self.recent.iter().copied()
    }
}
