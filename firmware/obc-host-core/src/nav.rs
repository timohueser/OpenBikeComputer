//! The host side of on-device route planning (#499): the resumable planner held across frames,
//! plus the shared commit/answer tail both hosts run when it finishes.

use crate::VecSink;
use obc_route::RouteSummary;

/// An in-flight route plan (#499): the resumable planner plus its caller-owned buffers and the
/// in-memory sink. A live host holds one and steps it **once per frame**, so the frame loop stays
/// fully interactive while a route computes (exactly how the board steps once per ride-loop pass);
/// `obc-sim`'s headless path loops it to completion via `plan_route` instead.
///
/// The A* table is the capped sim/LM20 size (`NAV_MAX_NODES` = 1536 ⇒ ~39 KB — the final
/// device's 40 kB nav budget, deliberately emulated so sim range = final-device range) and is
/// **heap-allocated zeroed**: an all-zero `NavScratch` is bit-identical to `new()` (its
/// `.bss`-placement contract; the first planner step resets it anyway), and `Box::new` would
/// first build the table on the stack — a silent trap on the wasm build's stack. The
/// ~9 KB planner (it owns the OBCR emitter across steps) is boxed for the same reason.
pub struct NavPlan {
    planner: Box<obc_route::NavPlanner>,
    scratch: Box<obc_route::nav::NavScratch>,
    tiles: obc_reader::NavTileCache,
    sink: VecSink,
}

impl NavPlan {
    /// Begin a plan for a drained [`NavRequest`](obc_app::NavRequest) under bike profile
    /// `profile_idx` (the rider's [`Settings::bike_profile_idx`](obc_app::Settings), N5 §8.6).
    pub fn start(req: &obc_app::NavRequest, profile_idx: u8) -> Self {
        NavPlan {
            planner: Box::new(obc_route::NavPlanner::new(req.from, req.to, req.name(), profile_idx)),
            // A zeroed heap allocation with no giant stack temp — obc-route owns the "all-zero *is*
            // `new()`" invariant (see `NavScratch::new_boxed`); the host just asks for one.
            scratch: obc_route::nav::NavScratch::new_boxed(),
            tiles: obc_reader::NavTileCache::new(),
            sink: VecSink::default(),
        }
    }

    /// Run **one bounded planner step** (the frame loop's per-frame unit). `Running` = keep going
    /// next frame; a terminal outcome is handed to [`finish_nav_plan`].
    pub fn step(&mut self, reader: &obc_reader::Reader) -> obc_route::Step {
        self.planner.step(reader, &mut self.scratch, &mut self.tiles, &mut self.sink)
    }

    /// The emitted OBCR bytes so far (complete once the planner reported `Done`).
    pub fn bytes(&self) -> &[u8] {
        self.sink.bytes()
    }

    /// The plan's cumulative tile-cache counters (misses = chunk reads).
    pub fn tile_stats(&self) -> obc_reader::NavCacheStats {
        self.tiles.stats()
    }
}

/// The slice of a host's route store the shared nav commit path ([`finish_nav_plan`]) drives:
/// write the reserved nav-route bytes, then re-feed the id-carrying catalog and invalidate the
/// change-gated active-route read. `obc-sim`'s folder-backed store and the in-memory
/// [`MemRouteStore`](crate::MemRouteStore) both implement it, so the commit sequence — the exact
/// rescan-then-answer order the device runs — lives in one place.
pub trait NavRouteStore {
    /// Persist the router's emitted OBCR as the reserved nav route (overwriting any previous
    /// plan), returning its session-stable id — or `None` on an I/O failure (the caller degrades
    /// to the generic routing-failure tier).
    fn write_nav_route(&mut self, bytes: &[u8]) -> Option<u16>;
    /// The route catalog (summaries), for [`App::set_routes_with_ids`](obc_app::App::set_routes_with_ids).
    fn catalog(&self) -> &[RouteSummary];
    /// Each catalog entry's session-stable id, parallel to [`catalog`](NavRouteStore::catalog).
    fn ids(&self) -> &[u16];
    /// Force the active route's bytes to re-read on the next sync even if the index is unchanged —
    /// a re-route rewrites the nav bytes **under** an unchanged catalog index.
    fn invalidate_active(&mut self);
}

/// Commit / report a finished plan and answer the app — the shared tail of the live hosts' stepped
/// path and `obc-sim`'s headless one-shot: on success write the reserved nav route, rescan +
/// re-feed the id-carrying catalog, and `notify_nav_result` (which swaps the planning screen for
/// the computed-route overview or the failure card), then hand the app the decimated shape
/// preview (#685 §4) — the emitted OBCR bytes are still in RAM here, so the ≤ 64-point copy is
/// decimated straight off them.
pub fn finish_nav_plan(
    app: &mut obc_app::App,
    store: &mut dyn NavRouteStore,
    outcome: Result<obc_route::RouteStats, obc_route::NavError>,
    sink_bytes: &[u8],
    tile_stats: obc_reader::NavCacheStats,
) {
    use obc_route::NavError;
    let result = outcome.and_then(|stats| {
        let id = store.write_nav_route(sink_bytes).ok_or(NavError::NoPath)?;
        app.set_routes_with_ids(store.catalog(), store.ids());
        // A re-route rewrites the nav bytes under an unchanged catalog index — force the
        // change-gated active-route read to re-open them.
        store.invalidate_active();
        // (eprintln! is a silent no-op on wasm32-unknown-unknown, so this stays unconditional.)
        eprintln!(
            "nav route: ok len={} m | tile-cache {} hit / {} miss (misses = chunk reads)",
            stats.total_distance_m, tile_stats.hits, tile_stats.misses
        );
        Ok(id)
    });
    if let Err(e) = &result {
        eprintln!("nav route: failed ({e:?})");
    }
    app.notify_nav_result(result);
    // The computed-route overview's shape preview (#685 §4), decimated host-side from the
    // just-committed bytes. After `notify_nav_result` (which activates the route and clears any
    // stale preview) so the copy keys to the fresh `active_route`. Skipped when the answer was
    // dropped (rider cancelled — no overview is up to draw it).
    if result.is_ok() && app.nav_preview_missing() {
        let src = obc_route::SliceSource(sink_bytes);
        if let Ok(idx) = obc_route::RouteIndex::read(&src) {
            let pts = obc_route::RouteReader::new(&idx, &src).preview_polyline::<{ obc_app::NAV_PREVIEW_MAX }>();
            app.set_nav_preview(&pts);
        }
    }
}
