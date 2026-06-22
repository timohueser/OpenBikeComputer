//! The on-screen **breadcrumb** — a bounded, two-tier record of where the rider has been.
//!
//! The durable ride log lives on the SD card ([`obc_route::track`]); this is the *picture*,
//! held in RAM so the map can draw the travelled path without ever re-reading storage. Its
//! two constraints are opposite, so it has two tiers:
//!
//! - [`recent`](Breadcrumb::recent) — a full-resolution sliding tail (the last ~2 km). The
//!   riding view is zoomed in, so the visible trail is here, and it's **never decimated** —
//!   just a fixed-length ring.
//! - [`spine`](Breadcrumb::spine) — the whole rest of the ride, held to a fixed point budget by
//!   **Visvalingam–Whyatt** simplification: when the budget is full, drop the single *least
//!   significant* vertex — the one whose [effective area](obc_route::tri_area_m2) (the triangle
//!   it makes with its two neighbours) is smallest, i.e. whose removal bends the line least.
//!   A straight run collapses toward its endpoints; a sharp bend is kept.
//!
//! Not a distance/perpendicular *tolerance* (issue #22): a global tolerance on a *growing*
//! track behaves badly under a fixed budget — once any section forces the tolerance up it
//! sticks, and every later point is then judged against it, so a long gently-curving stretch
//! gets drawn as one straight chord while stale early detail survives. Visvalingam has **no**
//! global tolerance — it always keeps exactly the budget and drops the globally-least-useful
//! point, so the budget **redistributes** to wherever the shape is. Removing a vertex widens its
//! neighbours' triangles, which protects them next time, so the points self-spread along the
//! ride instead of clustering. On a ride longer than the budget can hold at the route's fidelity
//! the spine simply coarsens — evenly, never collapsing one stretch to a line.
//!
//! The tiers are **disjoint**: a point lives in `recent` until it ages out of the ring, and
//! only *then* is it handed to the spine. So the two never cover the same ground, and the whole
//! trail draws as **one** chained polyline ([`points`](Breadcrumb::points)) — coarse for the old
//! part, full-resolution for the recent tail, with no doubled-up overlap. Both are
//! fixed-capacity `heapless` containers, so the renderer's polyline scratch can never overrun.

use heapless::{Deque, Vec};
use obc_route::{cos_lat, ground_dist_m, tri_area_m2_cl};

/// A 2-D point in microdegrees `(lon, lat)` — what the renderer projects.
type P = (i32, i32);

/// Full-resolution recent-tail capacity. At ≥[`RECENT_MIN_M`] spacing this covers the last
/// ~2 km of trail — more than the riding-zoom view ever shows.
const RECENT_CAP: usize = 256;
/// Minimum spacing (m) between recent-tail points — drops near-duplicate fixes (and a
/// stationary rider) so the ring spans real distance, not GPS jitter.
const RECENT_MIN_M: f32 = 4.0;

/// Whole-ride spine capacity (points). The spine holds exactly this many once warmed, so it
/// stays resident at ~6 KB regardless of ride length; the only lever for long-ride fidelity is
/// this number, at a linear RAM cost.
const SPINE_CAP: usize = 1024;

/// The travelled path drawn on the map: a full-res recent tail over a coarse whole-ride spine.
/// Owned by [`App`](crate::App) (it's kilobytes, so *not* the `Copy` [`Activity`](crate::Activity));
/// fed one accepted fix at a time from `App::tick`, cleared when a tracking session restarts.
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
    pub const fn new() -> Self {
        Breadcrumb { recent: Deque::new(), spine: Vec::new(), last_recent: None }
    }

    /// Forget the whole trail — called when a tracking session begins (load from Idle, or
    /// "Save & start new"); a "Swap route only" keeps it.
    pub fn clear(&mut self) {
        self.recent.clear();
        self.spine.clear();
        self.last_recent = None;
    }

    /// Whether the trail has anything to draw yet.
    pub fn is_empty(&self) -> bool {
        self.recent.is_empty() && self.spine.is_empty()
    }

    /// Add one accepted fix `(lon, lat)` to the trail. It enters the full-res `recent` ring;
    /// whatever ages out of the ring is handed to the coarse `spine` — so the two tiers stay
    /// disjoint and the whole trail is one continuous line.
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
    /// Visvalingam–Whyatt: while there's room just keep the point; once full, drop the
    /// least-significant interior vertex (smallest [`tri_area_m2_cl`]) and append the new one.
    ///
    /// Always keeps index 0 (the ride start) and the newest point, so the drawn line spans the
    /// whole ride and joins cleanly to `recent`. One O([`SPINE_CAP`]) scan per aged fix — no
    /// `sqrt`, no divide, ~1 Hz — negligible on the MCU.
    fn spine_push(&mut self, c: P) {
        if !self.spine.is_full() {
            let _ = self.spine.push(c);
            return;
        }
        let n = self.spine.len();
        if n < 2 {
            return; // degenerate budget (<2): keep the start, drop the rest
        }
        // `cos_lat` barely varies across one ride, so hoist it once for the whole scan rather
        // than per triangle. Find the interior vertex (1..n; the current last uses the incoming
        // `c` as its right neighbour) whose removal loses the least area.
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
        // last slot for the new point. Index 0 and the newest fix are preserved; budget stays full.
        for j in min_i..n - 1 {
            self.spine[j] = self.spine[j + 1];
        }
        self.spine[n - 1] = c;
    }

    /// The whole travelled path as **one** polyline, oldest→newest: the coarse spine chained to
    /// the full-res recent tail. The Map draws this in a single stroke, so the tiers never double up.
    pub fn points(&self) -> impl Iterator<Item = P> + '_ {
        self.spine.iter().copied().chain(self.recent.iter().copied())
    }

    /// Whole-ride spine points (coarse), oldest first — for introspection / tests.
    pub fn spine_iter(&self) -> impl Iterator<Item = P> + '_ {
        self.spine.iter().copied()
    }

    /// Recent-tail points (full resolution), oldest first — for introspection / tests.
    pub fn recent_iter(&self) -> impl Iterator<Item = P> + '_ {
        self.recent.iter().copied()
    }
}
