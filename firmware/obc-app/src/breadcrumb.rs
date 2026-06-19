//! The on-screen **breadcrumb** — a bounded, two-tier record of where the rider has been.
//!
//! The durable ride log lives on the SD card ([`obc_route::track`]); this is the *picture*,
//! held in RAM so the map can draw the travelled path without ever re-reading storage. Its
//! two constraints are opposite, so it has two tiers:
//!
//! - [`recent`](Breadcrumb::recent) — a full-resolution sliding tail (the last few km). While
//!   riding you follow yourself zoomed in, so the trail you actually *see* is here, and it's
//!   **never decimated** — just a fixed-length ring.
//! - [`spine`](Breadcrumb::spine) — the whole ride decimated to a fixed budget: a point every
//!   `spacing` metres, and when the budget fills it **halves the resolution and doubles the
//!   spacing**. So a 20 km ride keeps ~15 m spacing while a 150 km ride relaxes to ~120 m —
//!   coarse, but that's all you need in the zoomed-out overview where the spine is seen.
//!
//! The tiers are **disjoint**: a point lives in `recent` until it ages out of the ring, and
//! only *then* is it handed to `spine`. So the two never cover the same ground, and the whole
//! trail draws as **one** chained polyline ([`points`](Breadcrumb::points)) — coarse for the
//! old part, full-resolution for the recent tail, with no doubled-up overlap. Both are
//! fixed-capacity `heapless` containers, so the renderer's polyline scratch can never overrun.

use heapless::{Deque, Vec};
use obc_route::ground_dist_m;

/// A 2-D point in microdegrees `(lon, lat)` — what the renderer projects.
type P = (i32, i32);

/// Full-resolution recent-tail capacity. At ≥[`RECENT_MIN_M`] spacing this covers the last
/// ~2 km of trail — more than the riding-zoom view ever shows.
const RECENT_CAP: usize = 512;
/// Minimum spacing (m) between recent-tail points — drops near-duplicate fixes (and a
/// stationary rider) so the ring spans real distance, not GPS jitter.
const RECENT_MIN_M: f32 = 4.0;

/// Whole-ride spine capacity. With the doubling spacing below this stays resident at ~6 KB
/// regardless of ride length.
const SPINE_CAP: usize = 768;
/// Starting spine spacing (m). Doubles each time the spine fills, so the spacing self-tunes
/// to ride length (≈ `SPINE_START_M · 2^k` once `SPINE_CAP · 2^k` metres have been ridden).
const SPINE_START_M: f32 = 8.0;

/// The travelled path drawn on the map: a full-res recent tail over a coarse whole-ride spine.
/// Owned by [`App`](crate::App) (it's kilobytes, so *not* the `Copy` [`Activity`](crate::Activity));
/// fed one accepted fix at a time from `App::tick`, cleared when a tracking session restarts.
pub struct Breadcrumb {
    recent: Deque<P, RECENT_CAP>,
    spine: Vec<P, SPINE_CAP>,
    spine_spacing_m: f32,
    last_recent: Option<P>,
    last_spine: Option<P>,
}

impl Default for Breadcrumb {
    fn default() -> Self {
        Self::new()
    }
}

impl Breadcrumb {
    pub const fn new() -> Self {
        Breadcrumb {
            recent: Deque::new(),
            spine: Vec::new(),
            spine_spacing_m: SPINE_START_M,
            last_recent: None,
            last_spine: None,
        }
    }

    /// Forget the whole trail — called when a tracking session begins (load from Idle, or
    /// "Save & start new"); a "Swap route only" keeps it.
    pub fn clear(&mut self) {
        self.recent.clear();
        self.spine.clear();
        self.spine_spacing_m = SPINE_START_M;
        self.last_recent = None;
        self.last_spine = None;
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

    /// Add an aged-out point to the coarse whole-ride spine: one point per `spacing`, halving
    /// resolution + doubling the spacing when the budget fills.
    fn spine_push(&mut self, p: P) {
        if self.last_spine.is_none_or(|q| ground_dist_m(p, q) >= self.spine_spacing_m) {
            if self.spine.is_full() {
                self.thin_spine();
            }
            let _ = self.spine.push(p);
            self.last_spine = Some(p);
        }
    }

    /// The whole travelled path as **one** polyline, oldest→newest: the coarse spine (points
    /// that have aged out of the ring) chained to the full-res recent tail. The Map draws this
    /// in a single stroke, so the tiers never double up.
    pub fn points(&self) -> impl Iterator<Item = P> + '_ {
        self.spine.iter().chain(self.recent.iter()).copied()
    }

    /// Whole-ride spine points (coarse), oldest first — for introspection / tests.
    pub fn spine_iter(&self) -> impl Iterator<Item = P> + '_ {
        self.spine.iter().copied()
    }

    /// Recent-tail points (full resolution), oldest first — for introspection / tests.
    pub fn recent_iter(&self) -> impl Iterator<Item = P> + '_ {
        self.recent.iter().copied()
    }

    /// Halve the spine in place (keep every other point) and double the spacing, making room
    /// for more of a long ride without growing memory.
    fn thin_spine(&mut self) {
        let mut w = 0;
        let mut i = 0;
        while i < self.spine.len() {
            self.spine[w] = self.spine[i];
            w += 1;
            i += 2;
        }
        self.spine.truncate(w);
        self.spine_spacing_m *= 2.0;
    }
}
