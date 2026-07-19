//! The host side of on-device route planning (#499): the resumable planner held across frames,
//! plus the shared commit/answer tail both hosts run when it finishes.

use crate::VecSink;

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

/// An in-flight **detour** plan (#882): the same resumable-planner shape as [`NavPlan`], but the
/// planner carries the corridor blacklist and the plan's frozen request context (the
/// prefix/corridor anchor and rejoin distance) rides along to the splice.
pub struct DetourPlan {
    planner: Box<obc_route::NavPlanner>,
    scratch: Box<obc_route::nav::NavScratch>,
    tiles: obc_reader::NavTileCache,
    sink: VecSink,
    progress_m: u32,
    target_m: u32,
}

impl DetourPlan {
    /// Begin a detour plan for a drained [`DetourRequest`](obc_app::DetourRequest): resolve the
    /// rejoin coordinate at `target_m` on the resident active route and build the corridor over
    /// the skipped span. `None` when the route can't resolve the rejoin (vanished / unreadable) —
    /// the caller answers `DetourPlanned(Err)` immediately.
    pub fn start(req: &obc_app::DetourRequest, profile_idx: u8, orig: &obc_route::RouteReader) -> Option<Self> {
        let to = orig.position_at(req.target_m)?;
        let corridor = obc_route::Corridor::build(orig, req.progress_m, req.target_m);
        Some(DetourPlan {
            planner: Box::new(obc_route::NavPlanner::new_detour(
                req.from,
                (to.lon, to.lat),
                "Detour leg",
                profile_idx,
                corridor,
            )),
            scratch: obc_route::nav::NavScratch::new_boxed(),
            tiles: obc_reader::NavTileCache::new(),
            sink: VecSink::default(),
            progress_m: req.progress_m,
            target_m: req.target_m,
        })
    }

    /// Run one bounded planner step (the frame loop's per-frame unit).
    pub fn step(&mut self, reader: &obc_reader::Reader) -> obc_route::Step {
        self.planner.step(reader, &mut self.scratch, &mut self.tiles, &mut self.sink)
    }
}

/// A planned, **uncommitted** detour: the detour-only OBCR bytes plus the frozen splice context,
/// held host-side between `DetourPlanned` and the rider's `CommitDetour`/`CancelDetour`.
pub struct DetourReady {
    bytes: Vec<u8>,
    detour_len_m: u32,
    progress_m: u32,
    target_m: u32,
}

/// The detour plan finished (#882): answer the app — success hands over the preview figures
/// (`cost = detour length − skipped span length`, signed) and the decimated detour polyline
/// ([`App::set_detour_preview`](obc_app::App)), and returns the [`DetourReady`] the host holds
/// until commit/cancel; failure answers the typed error and holds nothing.
pub fn finish_detour_plan(
    app: &mut obc_app::App,
    outcome: Result<obc_route::RouteStats, obc_route::NavError>,
    plan: DetourPlan,
) -> Option<DetourReady> {
    match outcome {
        Ok(stats) => {
            let span_m = plan.target_m.saturating_sub(plan.progress_m);
            let preview = obc_app::DetourPreview {
                cost_delta_m: (stats.total_distance_m as i64 - span_m as i64) as i32,
                total_distance_m: stats.total_distance_m,
            };
            let bytes = plan.sink.into_bytes();
            // The preview polyline, decimated straight off the in-RAM detour OBCR.
            let src = obc_formats::io::SliceSource(&bytes);
            if let Ok(idx) = obc_route::RouteIndex::read(&src) {
                let pts = obc_route::RouteReader::new(&idx, &src).preview_polyline::<{ obc_app::NAV_PREVIEW_MAX }>();
                app.set_detour_preview(&pts);
            }
            eprintln!("detour plan: ok len={} m (Δ {:+} m)", stats.total_distance_m, preview.cost_delta_m);
            app.apply_event(obc_app::HostEvent::DetourPlanned(Ok(preview)));
            Some(DetourReady {
                bytes,
                detour_len_m: stats.total_distance_m,
                progress_m: plan.progress_m,
                target_m: plan.target_m,
            })
        }
        Err(e) => {
            eprintln!("detour plan: failed ({e:?})");
            app.apply_event(obc_app::HostEvent::DetourPlanned(Err(e)));
            None
        }
    }
}

/// Commit a planned detour (#882): stream-splice `original[0..anchor] + detour + original[rejoin..]`
/// into a derived OBCR, write it to the reserved computed-route slot, rescan + re-feed, and answer
/// `DetourCommitted`. On any failure the store is untouched and the app keeps its route.
///
/// `#[inline(never)]`: the splice runs one-shot here with its ~9 kB emitter frame — keep it out of
/// the dispatcher's frame (the same one-large-frame-at-a-time rule as the plan phases).
#[inline(never)]
pub fn finish_detour_commit(
    app: &mut obc_app::App,
    store: &mut dyn crate::RouteRepository,
    orig_index: Option<&obc_route::RouteIndex>,
    ready: Option<DetourReady>,
) {
    use obc_route::NavError;
    let result = (|| {
        let ready = ready.ok_or(NavError::NoPath)?;
        let orig_index = orig_index.ok_or(NavError::NoPath)?;
        let mut sink = VecSink::default();
        {
            // Both sources are in-RAM snapshots, so the splice completes before any store write —
            // self-splice (the active route already being the reserved slot) is safe.
            let orig_src = store.active_source().ok_or(NavError::NoPath)?;
            let orig = obc_route::RouteReader::new(orig_index, &orig_src);
            let det_src = obc_formats::io::SliceSource(&ready.bytes);
            let det_idx = obc_route::RouteIndex::read(&det_src).map_err(|_| NavError::NoPath)?;
            let det = obc_route::RouteReader::new(&det_idx, &det_src);
            obc_route::splice_detour(&orig, &det, ready.progress_m, ready.target_m, ready.detour_len_m, &mut sink)
                .map_err(|_| NavError::NoPath)?;
        }
        let id = store.write_nav_route(sink.bytes()).ok_or(NavError::NoPath)?;
        app.set_routes_with_meta(store.catalog(), store.ids(), &store.retention_metas());
        // The spliced bytes sit under the reserved slot's (possibly unchanged) id — force the
        // change-gated active-route read to re-open them.
        store.invalidate_active();
        eprintln!("detour commit: ok — spliced route id {id}");
        Ok(id)
    })();
    if let Err(e) = &result {
        eprintln!("detour commit: failed ({e:?})");
    }
    app.apply_event(obc_app::HostEvent::DetourCommitted(result));
}

/// Commit / report a finished plan and answer the app — the shared tail of the live hosts' stepped
/// path and `obc-sim`'s headless one-shot: on success write the reserved nav route, rescan +
/// re-feed the id-carrying catalog, and `notify_nav_result` (which swaps the planning screen for
/// the computed-route overview or the failure card), then hand the app the decimated shape
/// preview (#685 §4) — the emitted OBCR bytes are still in RAM here, so the ≤ 64-point copy is
/// decimated straight off them. Drives the store through [`RouteRepository`](crate::RouteRepository),
/// so the exact write→rescan→invalidate order lives in one place for every host.
pub fn finish_nav_plan(
    app: &mut obc_app::App,
    store: &mut dyn crate::RouteRepository,
    outcome: Result<obc_route::RouteStats, obc_route::NavError>,
    sink_bytes: &[u8],
    tile_stats: obc_reader::NavCacheStats,
) {
    use obc_route::NavError;
    let result = outcome.and_then(|stats| {
        let id = store.write_nav_route(sink_bytes).ok_or(NavError::NoPath)?;
        app.set_routes_with_meta(store.catalog(), store.ids(), &store.retention_metas());
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
    app.apply_event(obc_app::HostEvent::NavPlanned(result));
    // The computed-route overview's shape preview (#685 §4), decimated host-side from the
    // just-committed bytes. After the `NavPlanned` answer (which activates the route and clears any
    // stale preview) so the copy keys to the fresh `active_route`. Skipped when the answer was
    // dropped (rider cancelled — no overview is up to draw it).
    if result.is_ok() && app.nav_preview_missing() {
        let src = obc_formats::io::SliceSource(sink_bytes);
        if let Ok(idx) = obc_route::RouteIndex::read(&src) {
            let pts = obc_route::RouteReader::new(&idx, &src).preview_polyline::<{ obc_app::NAV_PREVIEW_MAX }>();
            app.set_nav_preview(&pts);
        }
    }
}
