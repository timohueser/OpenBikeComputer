//! The **route-corridor POI snapshot** the "Up ahead" timeline reads (epic #946, U2).
//!
//! [`CorridorScratch`] is the sibling of [`PoiScratch`](crate::screen::PoiScratch): one
//! [`App`](crate::App)-owned buffer holding a frozen snapshot of the map POIs sitting near the route
//! ahead, filled from [`Reader::corridor_pois`](obc_reader::Reader::corridor_pois) in the pre-draw
//! `prepare` pass and read-only everywhere else. No screen lives here — U3 draws it.
//!
//! # The frozen-snapshot contract (#115)
//!
//! Membership, order and distances are frozen on take. The snapshot is keyed by a
//! [`CorridorKey`] — the category filter **and** the progress anchor it was taken at — so live
//! progress advancing under the rider never re-runs the query and rows can never shift under the
//! cursor. It is re-taken only when the screen re-arms it: on entry, and on a filter change.
//!
//! # Storage decision (#425)
//!
//! A `heapless::Vec<CorridorPoi, 16>` is ~880 B. The [`Screen`](crate::screen::Screen) enum is a
//! union sized to its largest variant, held in a stack `Vec` in `.bss` — an inline snapshot would
//! multiply that across **every** slot. Held once in the App it costs the buffer once, and the
//! static-snapshot contract already forbids two live snapshots, so the single buffer loses nothing.
//! Exactly the reasoning `PoiScratch` records; the two never hold a snapshot at the same time in
//! practice, but they stay separate buffers because their record types differ.
//!
//! # The reader seam
//!
//! The query needs the streamed-map `Reader` (and the streamed route), which the board host builds
//! only when a frame needs it. [`pending`](CorridorScratch::pending) is what
//! [`App::base_needs_reader`](crate::App::base_needs_reader) adds to its answer, so the host builds
//! the `Reader` exactly until the snapshot lands and then stops — the same energy pattern as the
//! nearest-POI snapshot and the POI-detail hours read. A **failed** query counts as landed (an
//! empty list): retrying it every frame would re-run the query's worst case forever against a
//! corrupt POI section or a failing card. See [`CorridorScratch::prepare`].

use obc_reader::{CorridorPoi, PoiCategorySet, Reader, RoutePath, MAX_CORRIDOR_RESULTS};
use obc_route::RouteReader;

/// What a corridor snapshot is *for*: the category filter and the along-route progress it was
/// anchored at. Two snapshots with the same key are the same list, so re-arming with an unchanged
/// key is a no-op — which is what keeps the query off the per-frame path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CorridorKey {
    /// The categories the list shows ("Everything" is [`PoiCategorySet::ALL`]).
    pub filter: PoiCategorySet,
    /// Live route progress (m) at the moment the screen armed the request. Distances in the
    /// snapshot are relative to this, not to progress as it advances.
    pub anchor_m: u32,
}

/// What the Up-ahead timeline is currently **scoped to**: the rider's live category filter (app
/// state, reset on entry) and their persisted source preference (a settings row). Together they
/// decide which tables the list may walk and whether a corridor snapshot is wanted at all, so they
/// travel as one value — the pair cannot be passed apart and cannot drift.
///
/// Neither half lives on [`UpAheadScreen`](crate::screen::UpAheadScreen) any more (#1515 D4a): both
/// are edited from the context sheet *above* that screen, so a copy frozen inside it would be a
/// copy the rider's edit could not reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpAheadScope {
    /// The categories the list shows ("Everything" is [`PoiCategorySet::ALL`]).
    pub filter: PoiCategorySet,
    /// Which of the two source tables may feed the list — and, under
    /// [`WaypointsOnly`](crate::settings::UpAheadSource::WaypointsOnly), whether any snapshot is
    /// armed at all.
    pub source: crate::settings::UpAheadSource,
}

/// The [`App`](crate::App)-owned corridor snapshot. **One** buffer, shared by whatever screen is
/// showing the Up-ahead list — never owned by a [`Screen`](crate::screen::Screen) variant (see the
/// module docs).
pub struct CorridorScratch {
    /// The key a snapshot is *wanted* for — `None` when nothing is asking (the normal state: no
    /// Up-ahead screen is up, so the query never runs and the host never builds a `Reader` for it).
    want: Option<CorridorKey>,
    /// The key the held snapshot was taken for; `Some` even when the result is empty, so "queried,
    /// nothing ahead" is distinguishable from "not queried yet". `None` on a fresh/invalidated
    /// scratch.
    taken_for: Option<CorridorKey>,
    /// The corridor POIs for [`taken_for`](CorridorScratch::taken_for), ascending by along-route
    /// distance. Frozen once filled; the query owns the ordering.
    pois: heapless::Vec<CorridorPoi, MAX_CORRIDOR_RESULTS>,
}

impl CorridorScratch {
    /// An empty, disarmed scratch — nothing wanted, nothing taken.
    pub const fn new() -> Self {
        CorridorScratch { want: None, taken_for: None, pois: heapless::Vec::new() }
    }

    /// Ask for a snapshot of `key`. Idempotent: re-arming the key already held changes nothing (so a
    /// screen may call this every frame without re-querying), while a **different** key drops the
    /// stale rows immediately so no screen can draw a list that no longer matches its filter.
    pub fn arm(&mut self, key: CorridorKey) {
        self.want = Some(key);
        if self.taken_for != Some(key) {
            self.taken_for = None;
            self.pois.clear();
        }
    }

    /// Drop the held snapshot so the next `prepare` re-runs the query for the armed key — the
    /// "re-enter to refresh" half of the contract. Called when the Up-ahead screen opens.
    pub fn invalidate(&mut self) {
        self.taken_for = None;
        self.pois.clear();
    }

    /// Stop wanting a snapshot at all (the screen closed): drops the rows *and* the request, so the
    /// reader seam goes quiet.
    pub fn disarm(&mut self) {
        self.want = None;
        self.invalidate();
    }

    /// The key currently armed, if any.
    #[inline]
    pub fn armed(&self) -> Option<CorridorKey> {
        self.want
    }

    /// Whether a snapshot for `key` is held (possibly empty).
    #[inline]
    pub fn holds(&self, key: CorridorKey) -> bool {
        self.taken_for == Some(key)
    }

    /// Whether a query is armed but not yet satisfied — the fact the host reader-build seam reads.
    #[inline]
    pub fn pending(&self) -> bool {
        match self.want {
            Some(key) => !self.holds(key),
            None => false,
        }
    }

    /// The frozen snapshot, ascending by along-route distance. Empty before the first successful
    /// take (and for a genuinely empty corridor — [`holds`](Self::holds) tells the two apart).
    #[inline]
    pub fn entries(&self) -> &[CorridorPoi] {
        &self.pois
    }

    /// Number of entries in the held snapshot.
    #[inline]
    pub fn len(&self) -> usize {
        self.pois.len()
    }

    /// No entries held (either not queried yet, or nothing is up ahead).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.pois.is_empty()
    }

    /// Take the snapshot if one is armed, pending, and the frame carries both the map `Reader` and
    /// the streamed route. Called once per frame from the pre-draw `prepare` boundary.
    ///
    /// Two failures, deliberately handled differently:
    ///
    /// - **A missing input** (the host didn't build the `Reader` this frame, or no route is open
    ///   yet) is *not* an attempt — the scratch stays pending and retries next frame, which is what
    ///   keeps the seam asking until the inputs arrive.
    /// - **A query error** (a corrupt POI section, a failing card) **settles** the scratch on an
    ///   empty list, exactly as [`PoiScratch`](crate::screen::PoiScratch) does. Staying pending here
    ///   would re-run the *most expensive* form of the query on every rendered frame with the
    ///   `Reader` kept built — precisely the per-frame SD work the #115/#425 discipline exists to
    ///   forbid, in the one situation where it hurts most. One attempt per armed key; re-entry
    ///   ([`invalidate`](Self::invalidate)) or a filter change retries as usual.
    pub(crate) fn prepare(&mut self, reader: Option<&Reader>, route: Option<&RouteReader>) {
        let Some(key) = self.want else { return };
        if self.holds(key) {
            return; // already snapshotted for this key
        }
        let (Some(reader), Some(route)) = (reader, route) else { return };
        let path: &dyn RoutePath = route;
        if reader.corridor_pois(key.filter, path, key.anchor_m, &mut self.pois).is_err() {
            // Never freeze a half-filled list: an errored query settles as "queried, nothing".
            self.pois.clear();
        }
        self.taken_for = Some(key);
    }
}

impl Default for CorridorScratch {
    fn default() -> Self {
        CorridorScratch::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_reader::PoiCategory;

    fn key(filter: PoiCategorySet, anchor_m: u32) -> CorridorKey {
        CorridorKey { filter, anchor_m }
    }

    /// A fresh scratch wants nothing, so the reader seam stays quiet — the normal case, where no
    /// Up-ahead screen is up and the corridor query costs literally nothing.
    #[test]
    fn disarmed_scratch_is_never_pending() {
        let s = CorridorScratch::new();
        assert!(!s.pending(), "nothing armed ⇒ the host is never asked for a Reader");
        assert!(s.armed().is_none());
        assert!(s.is_empty());
    }

    /// Arming makes the scratch pending; a take satisfies it, and re-arming the same key does not
    /// re-query (the frozen contract — progress advancing does not move the anchor).
    #[test]
    fn arm_then_take_settles_and_stays_settled() {
        let mut s = CorridorScratch::new();
        let k = key(PoiCategorySet::ALL, 1_000);
        s.arm(k);
        assert!(s.pending(), "armed but not taken");
        s.taken_for = Some(k); // stand in for a successful query (no Reader in a unit test)
        assert!(!s.pending());
        s.arm(k);
        assert!(!s.pending(), "re-arming the held key is a no-op");
    }

    /// A different **filter** re-arms, and so does a different **anchor**: the key is the pair.
    /// Either change drops the stale rows immediately.
    #[test]
    fn a_changed_key_invalidates_both_ways() {
        let mut s = CorridorScratch::new();
        let k = key(PoiCategorySet::ALL, 1_000);
        s.arm(k);
        s.taken_for = Some(k);

        s.arm(key(PoiCategorySet::only(PoiCategory::Water), 1_000));
        assert!(s.pending(), "a filter change re-queries");
        assert!(s.is_empty(), "and drops the stale rows at once");

        let k2 = key(PoiCategorySet::ALL, 1_000);
        s.arm(k2);
        s.taken_for = Some(k2);
        s.arm(key(PoiCategorySet::ALL, 4_000));
        assert!(s.pending(), "a new progress anchor re-queries");
    }

    /// `invalidate` forces a re-take of the *same* key (screen re-entry); `disarm` also stops the
    /// request, so the reader seam goes quiet.
    #[test]
    fn invalidate_retakes_and_disarm_goes_quiet() {
        let mut s = CorridorScratch::new();
        let k = key(PoiCategorySet::ALL, 0);
        s.arm(k);
        s.taken_for = Some(k);
        assert!(!s.pending());

        s.invalidate();
        assert!(s.pending(), "re-entry re-queries the identical key");
        assert_eq!(s.armed(), Some(k), "the request survives an invalidate");

        s.disarm();
        assert!(!s.pending(), "a closed screen stops asking");
        assert!(s.armed().is_none());
    }
}
