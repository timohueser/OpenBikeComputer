//! The host side of on-device route planning (#499): the resumable planner held across frames,
//! plus the shared commit/answer tail both hosts run when it finishes.

use crate::trace::{DataKey, FeederCall, FeederKind, TraceSink};
use crate::VecSink;

/// An in-flight route plan (#499): the resumable planner plus its caller-owned buffers and the
/// in-memory sink. [`HostLoop`](crate::HostLoop) owns it under both host cadences: a live host runs
/// [`HostLoop::execute`](crate::HostLoop::execute) once per frame so the UI stays interactive,
/// while a scripted/headless host loops the same call until the plan reaches a terminal result.
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
    ///
    /// `elev` is the mounted map's terrain (EL7, epic #1068) — the emit phase fills each point's
    /// height from it. A host with no terrain hands in
    /// [`NullElevation`](obc_route::NullElevation) and the plan is exactly what it was before.
    pub fn step(&mut self, reader: &obc_reader::Reader, elev: &mut dyn obc_route::ElevationSource) -> obc_route::Step {
        self.planner.step(reader, &mut self.scratch, &mut self.tiles, elev, &mut self.sink)
    }

    /// The emitted OBCR bytes so far (complete once the planner reported `Done`).
    pub fn bytes(&self) -> &[u8] {
        self.sink.bytes()
    }

    /// The plan's cumulative graph-chunk and route-index cache counters.
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

    /// Run one bounded planner step (the frame loop's per-frame unit). `elev` fills the detour's
    /// own elevation exactly as it does a route plan's (EL7) — a spliced detour must not punch a
    /// flat span through the profile of the route it joins.
    pub fn step(&mut self, reader: &obc_reader::Reader, elev: &mut dyn obc_route::ElevationSource) -> obc_route::Step {
        self.planner.step(reader, &mut self.scratch, &mut self.tiles, elev, &mut self.sink)
    }
}

/// A planned, **uncommitted** detour: the detour-only OBCR bytes plus the frozen splice context,
/// held host-side between `DetourPlanned` and the rider's `CommitDetour`/`CancelDetour`.
///
/// The bytes and `rejoin_m` are already **trimmed to first tail contact** (#882) when the plan's
/// approach rode the route's own tail — see [`finish_detour_plan`]; so the splice context here is
/// exactly what commits, and `rejoin_m >= the chooser's target_m` (the chosen distance is a rejoin
/// *minimum*).
pub struct DetourReady {
    bytes: Vec<u8>,
    detour_len_m: u32,
    progress_m: u32,
    rejoin_m: u32,
    /// The plan's own [`RouteStats::has_elevation`](obc_route::RouteStats) — did the mounted
    /// terrain answer for this detour? Carried (never re-derived from the bytes: `0 m` is a real
    /// height) so the splice knows whether the leg's stored heights are sampled terrain to keep or
    /// the `0` placeholder to replace.
    has_elevation: bool,
}

/// The detour plan finished (#882): the preview figures the typed
/// [`NavigatorOutcome::DetourFinished`](obc_app::navigator::NavigatorOutcome) carries
/// (`cost = detour length − skipped span length`, signed), the decimated detour polyline
/// ([`App::set_detour_preview`](obc_app::App)), and the [`DetourReady`] the executor holds until the
/// rider commits or cancels. A failure reports the typed error and holds nothing.
///
/// `orig` is the resident original route: when present, a successful plan is trimmed to its first
/// sustained contact with the route tail past `target_m`
/// ([`trim_detour_to_tail`](obc_route::trim_detour_to_tail)), so the preview polyline and cost line
/// already describe the shortened detour and the splice rejoins at that farther point. `None` (or a
/// trim that doesn't bite) keeps the untrimmed bytes, the planner length, and the chosen `target_m`.
pub fn plan_detour_preview(
    app: &mut obc_app::App,
    outcome: Result<obc_route::RouteStats, obc_route::NavError>,
    plan: DetourPlan,
    orig: Option<&obc_route::RouteReader>,
    trace: &mut dyn TraceSink,
) -> (Option<DetourReady>, Result<obc_app::DetourPreview, obc_route::NavError>) {
    match outcome {
        Ok(stats) => {
            let mut bytes = plan.sink.into_bytes();
            // Default (no trim): rejoin at the chosen minimum, the planner's summed edge length and
            // the plan's own dead-banded climb.
            let mut rejoin_m = plan.target_m;
            let mut detour_len_m = stats.total_distance_m;
            let mut detour_ascent_m = stats.total_ascent_m;

            // Advance the rejoin to the detour's first sustained contact with the route tail: A*
            // legally rides the future route's own road to the goal (the tail past `target_m` is not
            // blacklisted), and the splice would append a tail that immediately retraces it. The
            // trim re-emits `detour[0..=contact]` into a fresh buffer; on any miss we keep today's.
            let trimmed = orig.and_then(|orig| {
                let src = obc_formats::io::SliceSource(&bytes);
                let didx = obc_route::RouteIndex::read(&src).ok()?;
                let det = obc_route::RouteReader::new(&didx, &src);
                let mut trim_sink = VecSink::default();
                match obc_route::trim_detour_to_tail(orig, &det, plan.target_m, stats.has_elevation, &mut trim_sink) {
                    Ok(Some(o)) => Some((o, trim_sink.into_bytes())),
                    _ => None,
                }
            });
            if let Some((o, tbytes)) = trimmed {
                let saved = (stats.total_distance_m as i64 + (o.rejoin_m as i64 - plan.target_m as i64)
                    - o.detour_len_m as i64)
                    .max(0) as u32;
                eprintln!("detour plan: trimmed to first tail contact at {} m (−{saved} m)", o.rejoin_m);
                rejoin_m = o.rejoin_m;
                detour_len_m = o.detour_len_m;
                detour_ascent_m = o.ascent_m;
                bytes = tbytes;
            }

            // Cost = detour length − skipped span (`rejoin_m − progress_m`); the trim lengthens the
            // skipped span and shortens the detour, so both terms improve the figure honestly.
            //
            // The climb side of the preview is the leg's *own* ascent (EL7-sampled, dead-banded);
            // the replaced span's ascent is read app-side off the resident route profile, which is
            // where `Profile::ascent_between_m` already lives. `None` when the terrain never
            // answered — the explicit bit, so a genuinely flat detour still shows `+0`.
            let preview = obc_app::DetourPreview {
                cost_delta_m: (detour_len_m as i64 - (rejoin_m as i64 - plan.progress_m as i64)) as i32,
                total_distance_m: detour_len_m,
                rejoin_m,
                ascent_m: stats.has_elevation.then_some(detour_ascent_m),
            };
            // The preview polyline, decimated straight off the (possibly trimmed) in-RAM detour OBCR.
            let src = obc_formats::io::SliceSource(&bytes);
            if let Ok(idx) = obc_route::RouteIndex::read(&src) {
                let pts = obc_route::RouteReader::new(&idx, &src).preview_polyline::<{ obc_app::NAV_PREVIEW_MAX }>();
                app.set_detour_preview(&pts);
                trace.feeder(FeederCall::new(
                    FeederKind::DetourPreview,
                    DataKey::from("host.detour-preview"),
                    pts.len(),
                ));
            }
            eprintln!("detour plan: ok len={detour_len_m} m (Δ {:+} m)", preview.cost_delta_m);
            let ready = DetourReady {
                bytes,
                detour_len_m,
                progress_m: plan.progress_m,
                rejoin_m,
                has_elevation: stats.has_elevation,
            };
            (Some(ready), Ok(preview))
        }
        Err(e) => {
            eprintln!("detour plan: failed ({e:?})");
            (None, Err(e))
        }
    }
}

/// Commit a planned detour (#882): stream-splice `original[0..anchor] + detour + original[rejoin..]`
/// into a derived OBCR, write it to the reserved computed-route slot, rescan + re-feed, and report
/// the spliced identity — what the executor turns into
/// [`NavigatorOutcome::DetourCommitted`](obc_app::navigator::NavigatorOutcome). On any failure the
/// store is untouched and the app keeps its route.
///
/// `#[inline(never)]`: the splice runs one-shot here with its ~9 kB emitter frame — keep it out of
/// the executor's frame (the same one-large-frame-at-a-time rule as the plan phases).
#[inline(never)]
pub fn commit_detour(
    app: &mut obc_app::App,
    store: &mut dyn crate::RouteRepository,
    orig_index: Option<&obc_route::RouteIndex>,
    ready: Option<DetourReady>,
    trace: &mut dyn TraceSink,
) -> Result<obc_app::CatalogObjectId, obc_route::NavError> {
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
            obc_route::splice_detour(
                &orig,
                &det,
                ready.progress_m,
                ready.rejoin_m,
                ready.detour_len_m,
                ready.has_elevation,
                &mut sink,
            )
            .map_err(|_| NavError::NoPath)?;
        }
        let id = store.write_nav_route(sink.bytes()).ok_or(NavError::NoPath)?;
        let metas = store.retention_metas();
        app.set_routes_with_meta(store.catalog(), store.ids(), &metas);
        trace.feeder(FeederCall::new(FeederKind::RouteCatalog, DataKey::from("host.routes"), store.catalog().len()));
        trace.feeder(FeederCall::new(FeederKind::RouteRetention, DataKey::from("host.route-retention"), metas.len()));
        // The spliced bytes sit under the reserved slot's (possibly unchanged) id — force the
        // change-gated active-route read to re-open them.
        store.invalidate_active();
        eprintln!("detour commit: ok — spliced route id {id}");
        Ok(id)
    })();
    if let Err(e) = &result {
        eprintln!("detour commit: failed ({e:?})");
    }
    result
}

/// Commit a finished plan — the shared tail of the live hosts' stepped path and `obc-sim`'s
/// headless one-shot: on success write the reserved nav route, rescan + re-feed the id-carrying
/// catalog, and report the committed identity, which the executor turns into
/// [`NavigatorOutcome::PlanFinished`](obc_app::navigator::NavigatorOutcome). Drives the store
/// through [`RouteRepository`](crate::RouteRepository), so the exact write→rescan→invalidate order
/// lives in one place for every host.
///
/// The computed overview's shape preview (#685 §4) is not decimated here: it is answered from the
/// next plan's `derived_needs` key, against the identity this commit reports.
pub fn commit_nav_plan(
    app: &mut obc_app::App,
    store: &mut dyn crate::RouteRepository,
    outcome: Result<obc_route::RouteStats, obc_route::NavError>,
    sink_bytes: &[u8],
    tile_stats: obc_reader::NavCacheStats,
    trace: &mut dyn TraceSink,
) -> Result<obc_app::CatalogObjectId, obc_route::NavError> {
    use obc_route::NavError;
    let result = outcome.and_then(|stats| {
        let id = store.write_nav_route(sink_bytes).ok_or(NavError::NoPath)?;
        let metas = store.retention_metas();
        app.set_routes_with_meta(store.catalog(), store.ids(), &metas);
        trace.feeder(FeederCall::new(FeederKind::RouteCatalog, DataKey::from("host.routes"), store.catalog().len()));
        trace.feeder(FeederCall::new(FeederKind::RouteRetention, DataKey::from("host.route-retention"), metas.len()));
        // A re-route rewrites the nav bytes under an unchanged catalog index — force the
        // change-gated active-route read to re-open them.
        store.invalidate_active();
        // (eprintln! is a silent no-op on wasm32-unknown-unknown, so this stays unconditional.)
        eprintln!(
            "nav route: ok len={} m | graph {} hit / {} read, index {} hit / {} read, {} source reads total",
            stats.total_distance_m,
            tile_stats.hits,
            tile_stats.misses,
            tile_stats.index_hits,
            tile_stats.index_misses,
            tile_stats.source_reads()
        );
        Ok(id)
    });
    if let Err(e) = &result {
        eprintln!("nav route: failed ({e:?})");
    }
    result
}
