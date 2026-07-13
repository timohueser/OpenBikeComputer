//! The POI create-route flow (epic #116, R4): the detail's press → "Create a route?" confirm, the
//! one-shot [`NavRequest`] seam the host drains ([`App::take_nav_request`]), and the host's answer
//! ([`App::notify_nav_result`]) — success swaps the confirm for the computed-route overview
//! (activated, length only), the locked two failure tiers swap it for the failure card, and the
//! overview's accept honours the ride state (idle → start; tracking → the existing save/swap
//! prompt). Screens are driven through the real gesture path with the POI harness's fixture map,
//! exactly like `poi.rs`.

use embedded_graphics::pixelcolor::Rgb888;
use obc_app::screen::{needle_region, Screen};
use obc_app::{App, AppState, Fix, Gesture, IdleReturn, InputClock, Mode, NavRequest, RouteSummary, Settings};
use obc_reader::{rgb565_to_rgb888, BBox, MapCache, MapTables, Reader, SliceSource};
use obc_route::NavError;
use obcm_testkit::{build_poi_map, PoiSpec};

mod common;
use common::Buf;

/// The fixture map bbox `(min_lon, min_lat, max_lon, max_lat)` and query point — `poi.rs`'s.
const BBOX: (i32, i32, i32, i32) = (7_000_000, 43_000_000, 8_000_000, 44_000_000);
const POS: (i32, i32) = (7_500_000, 43_500_000);
/// The nearest Water POI's coordinate (lon, lat µdeg) — what the confirm's request targets.
const POI: (i32, i32) = (7_500_000, 43_500_500);

/// A v7+ map with one named Water POI due north of [`POS`] and one unnamed Campsite (subtype 5 →
/// the "Campsite" fallback label), for the name-fallback path.
fn fixture() -> Vec<u8> {
    let water = vec![PoiSpec { lat: POI.1, lon: POI.0, subtype: 1, name: "Fountain North".into(), hours_ref: 0xFFFF }];
    let campsite =
        vec![PoiSpec { lat: 43_501_000, lon: 7_500_000, subtype: 5, name: String::new(), hours_ref: 0xFFFF }];
    build_poi_map(BBOX, 512, &[(1, water), (2, campsite)])
}

/// Render one frame against the fixture into `buf` (fills the lazy POI snapshot, like the sim's
/// draw).
fn render_into(app: &mut App, bytes: &[u8], buf: &mut Buf) {
    let cache = MapCache::new();
    let src = SliceSource(bytes);
    let tables = MapTables::parse(&src).expect("valid fixture");
    let reader = Reader::new(&src, &tables, &cache);
    app.render_frame(buf, &reader, None, 240.0, 320.0, |c| {
        let (r, g, b) = rgb565_to_rgb888(c);
        Rgb888::new(r, g, b)
    });
}

/// [`render_into`] a throwaway frame, for the tests that only need the render's side effects.
fn render(app: &mut App, bytes: &[u8]) {
    let mut buf = Buf::new(240, 320);
    render_into(app, bytes, &mut buf);
}

/// Walk an idle app from Home to the nearest Water POI's **detail**: Menu → POIs → Water list,
/// one render to take the snapshot, press into the detail.
fn open_detail(app: &mut App, bytes: &[u8]) {
    app.state.user_fix = Some(Fix::at(POS.1, POS.0));
    app.apply_gesture(Gesture::BackHold); // Home → Menu
    app.apply_gesture(Gesture::Turn(2)); // Routes → Rides → POIs
    app.apply_gesture(Gesture::Press); // → category list (Water first)
    app.apply_gesture(Gesture::Press); // → POI list
    render(app, bytes); // lazy snapshot fills
    app.apply_gesture(Gesture::Press); // → detail
    assert!(matches!(app.top_screen(), Screen::PoiDetail(_)));
}

/// Drive the detail into the confirm and press *Create route*, returning the drained request.
/// The confirm swaps itself for the **planning** screen (#499) — the spinner the host answers into.
fn request_route(app: &mut App) -> NavRequest {
    app.apply_gesture(Gesture::Press); // detail → confirm
    assert!(matches!(app.top_screen(), Screen::NavConfirm(_)), "detail press opens the confirm");
    app.apply_gesture(Gesture::Press); // Create route (row 0)
    assert!(matches!(app.top_screen(), Screen::NavPlanning(_)), "accepting swaps to the planning screen");
    app.take_nav_request().expect("Create route records the one-shot request")
}

/// A one-route catalog standing in for the rescan after the host wrote `_nav.obcr` — the summary
/// the emitted OBCR would scan to, under durable id 7.
fn nav_catalog(app: &mut App) {
    let mut name = heapless::String::<48>::new();
    let _ = name.push_str("Fountain North");
    let sum = RouteSummary {
        name,
        distance_km: 0,
        climb_m: 0,
        bbox: BBox { min_lon: POS.0, min_lat: POS.1, max_lon: POI.0, max_lat: POI.1 },
        start_lon: POS.0,
        start_lat: POS.1,
    };
    app.set_routes_with_ids(&[sum], &[7]);
}

#[test]
fn detail_press_confirm_create_records_the_request() {
    let bytes = fixture();
    let mut app = App::new_idle(AppState::new(POS.0, POS.1, 0.05));
    open_detail(&mut app, &bytes);
    let req = request_route(&mut app);
    assert_eq!(req.from, POS, "the route starts at the rider's fix");
    assert_eq!(req.to, POI, "…and ends at the POI");
    assert_eq!(req.name(), "Fountain North", "named POIs title the route with their stored name");
    assert!(app.take_nav_request().is_none(), "the request is a one-shot");
}

#[test]
fn unnamed_poi_falls_back_to_the_subtype_label() {
    let bytes = fixture();
    let mut app = App::new_idle(AppState::new(POS.0, POS.1, 0.05));
    app.state.user_fix = Some(Fix::at(POS.1, POS.0));
    app.apply_gesture(Gesture::BackHold);
    app.apply_gesture(Gesture::Turn(2));
    app.apply_gesture(Gesture::Press);
    app.apply_gesture(Gesture::Turn(1)); // Water → Campsite
    app.apply_gesture(Gesture::Press);
    render(&mut app, &bytes);
    app.apply_gesture(Gesture::Press); // → detail (the unnamed campsite)
    let req = request_route(&mut app);
    assert_eq!(req.name(), "Campsite", "an unnamed POI titles the route with its subtype label");
}

#[test]
fn confirm_cancel_and_back_return_to_the_detail() {
    let bytes = fixture();
    let mut app = App::new_idle(AppState::new(POS.0, POS.1, 0.05));
    open_detail(&mut app, &bytes);
    app.apply_gesture(Gesture::Press); // → confirm
    app.apply_gesture(Gesture::Turn(1)); // → Cancel
    app.apply_gesture(Gesture::Press);
    assert!(matches!(app.top_screen(), Screen::PoiDetail(_)), "Cancel returns to the detail");
    assert!(app.take_nav_request().is_none(), "cancel records nothing");

    app.apply_gesture(Gesture::Press); // → confirm again
    app.apply_gesture(Gesture::Back);
    assert!(matches!(app.top_screen(), Screen::PoiDetail(_)), "Back = Cancel");
}

#[test]
fn success_activates_and_opens_the_computed_overview() {
    let bytes = fixture();
    let mut app = App::new_idle(AppState::new(POS.0, POS.1, 0.05));
    open_detail(&mut app, &bytes);
    let _req = request_route(&mut app);

    // The host's success path: catalog rescanned (the committed nav route under id 7), then the
    // answer — resolved by durable id, exactly like an upload notification.
    nav_catalog(&mut app);
    let _ = app.take_dirty();
    app.notify_nav_result(Ok(7));
    assert!(matches!(app.top_screen(), Screen::RouteOverview(_)), "success swaps the confirm for the overview");
    assert_eq!(app.activity.active_route, Some(0), "the computed route activates for the preview");
    assert!(app.take_dirty().map, "the swap repaints");

    // Accept from Idle = the normal route start.
    app.apply_gesture(Gesture::Press);
    assert!(matches!(app.top_screen(), Screen::Map(_)), "accept starts the ride");
    assert_eq!(app.mode(), Mode::Riding);
    assert!(app.activity.is_tracking(), "starting from Idle begins a session");
}

#[test]
fn overview_back_restores_the_previous_route_and_returns_to_the_detail() {
    let bytes = fixture();
    let mut app = App::new_idle(AppState::new(POS.0, POS.1, 0.05));
    open_detail(&mut app, &bytes);
    let _req = request_route(&mut app);
    nav_catalog(&mut app);
    app.notify_nav_result(Ok(7));
    assert_eq!(app.activity.active_route, Some(0));

    app.apply_gesture(Gesture::Back); // cancel the overview
    assert!(
        matches!(app.top_screen(), Screen::PoiDetail(_)),
        "the overview replaced the confirm, so Back lands on the detail"
    );
    assert_eq!(app.activity.active_route, None, "cancel restores what was loaded before (nothing)");
}

#[test]
fn mid_ride_accept_opens_the_save_swap_prompt() {
    let bytes = fixture();
    let mut app = App::new_idle(AppState::new(POS.0, POS.1, 0.05));
    // Ride first: a tracking session over the (single-entry) catalog.
    nav_catalog(&mut app);
    app.state.user_fix = Some(Fix::at(POS.1, POS.0));
    app.apply_gesture(Gesture::Press); // Home → Menu (Routes selected)
    app.apply_gesture(Gesture::Press); // press Routes → Route menu
    app.apply_gesture(Gesture::Press); // → overview
    app.apply_gesture(Gesture::Press); // → START RIDE
    assert!(app.activity.is_tracking());
    let session = app.activity.session;

    // Mid-ride: Menu → POIs → detail → confirm → create → (host answers).
    app.apply_gesture(Gesture::BackHold);
    app.apply_gesture(Gesture::Turn(2));
    app.apply_gesture(Gesture::Press);
    app.apply_gesture(Gesture::Press);
    render(&mut app, &bytes);
    app.apply_gesture(Gesture::Press); // → detail
    let _req = request_route(&mut app);
    app.notify_nav_result(Ok(7));
    assert!(matches!(app.top_screen(), Screen::RouteOverview(_)));

    // Accept while tracking → the existing save/swap prompt, session untouched.
    app.apply_gesture(Gesture::Press);
    assert!(matches!(app.top_screen(), Screen::RouteSwap(_)), "mid-ride accept opens the save/swap prompt");
    assert_eq!(app.activity.session, session, "the recording session is untouched until the prompt decides");

    // "Swap route" keeps the session and drops onto the riding Map.
    app.apply_gesture(Gesture::Press);
    assert!(matches!(app.top_screen(), Screen::Map(_)));
    assert_eq!(app.activity.session, session, "swap keeps the session");
}

#[test]
fn failure_tiers_swap_the_confirm_for_the_right_card() {
    let bytes = fixture();
    // With no distance cap, **exhaustion is the range tier**: running out of the router's fixed
    // table is the device's honest "too far", so it (and only it) shows "Too far to route here";
    // everything else is the generic "Couldn't find a route."
    for (err, tier, expect_too_far) in
        [(NavError::Exhausted, "range (exhausted)", true), (NavError::NoPath, "generic", false)]
    {
        let mut app = App::new_idle(AppState::new(POS.0, POS.1, 0.05));
        open_detail(&mut app, &bytes);
        let _req = request_route(&mut app);
        app.notify_nav_result(Err(err));
        match app.top_screen() {
            Screen::NavFail(card) => assert_eq!(
                card.shows_too_far(),
                expect_too_far,
                "{tier}: the card must show the {} tier",
                if expect_too_far { "range" } else { "generic" }
            ),
            _ => panic!("{tier}: failure swaps in the card"),
        }
        assert_eq!(app.activity.active_route, None, "{tier}: nothing activates on failure");
        app.apply_gesture(Gesture::Press);
        assert!(matches!(app.top_screen(), Screen::PoiDetail(_)), "{tier}: any press dismisses to the detail");
    }
}

#[test]
fn create_without_any_position_degrades_to_the_generic_tier() {
    let bytes = fixture();
    let mut app = App::new_idle(AppState::new(POS.0, POS.1, 0.05));
    open_detail(&mut app, &bytes);
    app.apply_gesture(Gesture::Press); // → confirm
    app.state.user_fix = None; // genuinely no position (can't happen after a snapshot, but locked to degrade)
    app.apply_gesture(Gesture::Press); // Create route
    assert!(matches!(app.top_screen(), Screen::NavFail(_)), "no position ⇒ the generic failure tier, no request");
    assert!(app.take_nav_request().is_none(), "nothing was asked of the host");
}

#[test]
fn unresolvable_id_degrades_to_the_generic_tier() {
    let bytes = fixture();
    let mut app = App::new_idle(AppState::new(POS.0, POS.1, 0.05));
    open_detail(&mut app, &bytes);
    let _req = request_route(&mut app);
    // The host claims success under an id the (empty) catalog doesn't hold — a failed rescan.
    app.notify_nav_result(Ok(42));
    assert!(matches!(app.top_screen(), Screen::NavFail(_)), "an unresolvable id is a failure, not a wrong route");
}

#[test]
fn result_without_a_planning_screen_on_stack_is_dropped() {
    let mut app = App::new_idle(AppState::new(POS.0, POS.1, 0.05));
    nav_catalog(&mut app);
    let _ = app.take_dirty();
    app.notify_nav_result(Ok(7));
    assert!(matches!(app.top_screen(), Screen::Home(_)), "no planning screen up ⇒ the answer is dropped");
    assert_eq!(app.activity.active_route, None, "…and nothing activates behind the rider's back");
    assert!(!app.take_dirty().map, "…and nothing repaints");
}

#[test]
fn back_on_planning_cancels_cleanly() {
    let bytes = fixture();
    let mut app = App::new_idle(AppState::new(POS.0, POS.1, 0.05));
    open_detail(&mut app, &bytes);
    let _req = request_route(&mut app);
    assert!(!app.take_nav_cancel(), "no cancel recorded yet");

    // Back mid-plan: straight back to the POI detail — no failure card — and the host's
    // cancel one-shot rings so it aborts the plan + discards the partial file.
    app.apply_gesture(Gesture::Back);
    assert!(matches!(app.top_screen(), Screen::PoiDetail(_)), "cancel returns to the detail");
    assert!(app.take_nav_cancel(), "the cancel one-shot rings for the host");
    assert!(!app.take_nav_cancel(), "…exactly once");

    // A late answer (the host may have finished the step before draining the cancel — it
    // shouldn't notify after an abort, but stay defensive) finds no planning screen: dropped.
    nav_catalog(&mut app);
    app.notify_nav_result(Ok(7));
    assert!(matches!(app.top_screen(), Screen::PoiDetail(_)), "a post-cancel answer is dropped");
    assert_eq!(app.activity.active_route, None, "nothing activates after a cancel");
}

#[test]
fn planning_spinner_throttles_repaints_to_its_cadence() {
    use obc_app::screen::NavPlanningScreen;
    // #500: during a plan the ride loop ticks the screen once per planner step — every ~8 ms —
    // and each claimed repaint costs a full chrome render + push (~40 ms on glass). The spinner
    // must claim `changed` at most once per its 66 ms frame cadence, not per tick, or the
    // repaints starve the plan they're decorating. (The needle still advances by real elapsed
    // time, so a throttled frame just shows a larger sweep.)
    let mut s = NavPlanningScreen::new("Fountain North");
    let first = s.tick_timers(1_000, 240, 320); // anchors the clocks; nothing elapsed yet
    assert!(!first.changed, "no time elapsed, nothing to repaint");
    assert_eq!(first.next_wake_ms, Some(66), "the spinner keeps its frame cadence armed");
    assert_eq!(first.region, Some(needle_region(240, 320)), "the spinner reports its needle-disc region");

    // 100 ride-loop passes at 8 ms: ~1 claim per ceil(66/8)·8 = 72 ms window, not 100.
    let claims = (1..=100).filter(|i| s.tick_timers(1_000 + i * 8, 240, 320).changed).count();
    assert!((10..=13).contains(&claims), "expected ~800 ms / 72 ms ≈ 11 repaints, got {claims}");
}

#[test]
fn needle_region_covers_the_spin() {
    // The region-scoped repaint's contract (#500 follow-up): while a plan runs, successive
    // full-repaint frames differ **only inside** the reported `needle_region`. The on-device
    // repaint clips to that region and *discards* every write outside it, so a changing pixel
    // out there would go stale on glass — this sweep is what makes the clip safe.
    use embedded_graphics::prelude::Point;
    let bytes = fixture();
    let mut app = App::new_idle(AppState::new(POS.0, POS.1, 0.05));
    open_detail(&mut app, &bytes);
    let _ = request_route(&mut app);
    let region = needle_region(240, 320);

    let mut prev = Buf::new(240, 320);
    app.advance_animations(InputClock(40)); // anchors the spinner clocks (dt = 0: no sweep yet)
    render_into(&mut app, &bytes, &mut prev);
    // Step the spinner through more than a full revolution in odd, cadence-beating increments
    // (81 ms · 20 ≈ 1.6 s ≈ 1.1 revolutions at 240°/s), diffing each frame against the last.
    for i in 1..=20u32 {
        app.advance_animations(InputClock(40 + i * 81));
        let mut cur = Buf::new(240, 320);
        render_into(&mut app, &bytes, &mut cur);
        let mut diffs = 0;
        for y in 0..320 {
            for x in 0..240 {
                if cur.get(x, y) != prev.get(x, y) {
                    assert!(
                        region.contains(Point::new(x, y)),
                        "pixel ({x},{y}) changed outside needle_region at step {i}"
                    );
                    diffs += 1;
                }
            }
        }
        assert!(diffs > 0, "step {i}: the needle must actually have swept (vacuous diff)");
        prev = cur;
    }
}

#[test]
fn clipped_replay_matches_the_full_render_inside_the_region() {
    // The Canvas-level primitive rejection (`App::set_render_clip`): a clipped repaint replayed
    // over the previous frame must be byte-identical to a full render **inside** the region —
    // outside it the device framebuffer discards writes (obc-platform's clip tests), so inside
    // is the half the app owns. Rejection being conservative (only fully-disjoint primitives
    // skip) is exactly what this pins: a wrongly-rejected straddler would leave stale needle
    // pixels in the region.
    use embedded_graphics::prelude::Point;
    let bytes = fixture();
    let mut app = App::new_idle(AppState::new(POS.0, POS.1, 0.05));
    open_detail(&mut app, &bytes);
    let _ = request_route(&mut app);
    let region = needle_region(240, 320);

    app.advance_animations(InputClock(40)); // anchor the spinner clocks
    let mut prev = Buf::new(240, 320);
    render_into(&mut app, &bytes, &mut prev);
    for i in 1..=6u32 {
        app.advance_animations(InputClock(40 + i * 81));
        // The reference: a full render of the current state.
        let mut full = Buf::new(240, 320);
        render_into(&mut app, &bytes, &mut full);
        // The clipped replay over the previous frame (what the device does each spinner tick).
        let mut clipped = Buf::new(240, 320);
        clipped.px.copy_from_slice(&prev.px);
        app.set_render_clip(Some(region));
        render_into(&mut app, &bytes, &mut clipped);
        for y in 0..320 {
            for x in 0..240 {
                if region.contains(Point::new(x, y)) {
                    assert_eq!(
                        clipped.get(x, y),
                        full.get(x, y),
                        "step {i}: clipped replay diverges from the full render at ({x},{y})"
                    );
                }
            }
        }
        prev = full;
    }
}

#[test]
fn planning_region_scopes_take_dirty() {
    // The seam the board's clipped repaint hangs off: a spinner tick's dirt drains as
    // `map: true` **with** the needle region; any full-frame demand in the same window folds
    // the region away; and before a first frame states the panel size, the spinner abstains.
    let bytes = fixture();
    let mut app = App::new_idle(AppState::new(POS.0, POS.1, 0.05));
    open_detail(&mut app, &bytes);
    let _ = request_route(&mut app);
    let _ = app.take_dirty(); // drain the navigation's own dirt
    app.advance_animations(InputClock(1_000)); // anchors the spinner clocks — nothing fired yet
    assert_eq!(app.take_dirty(), obc_app::Dirty::CLEAN, "the anchoring tick claims nothing");

    app.advance_animations(InputClock(1_066)); // one spinner cadence later: the repaint claim
    let d = app.take_dirty();
    assert!(d.map, "the spinner's claim dirties the map plane");
    assert_eq!(d.region, Some(needle_region(240, 320)), "…scoped to the needle disc");

    // A stack-changing gesture (Back = cancel → pop) in the same window is full-frame dirt: it
    // overrides the region even though a spinner tick also fired.
    app.advance_animations(InputClock(1_140));
    app.apply_gesture(Gesture::Back);
    let d = app.take_dirty();
    assert!(d.map);
    assert_eq!(d.region, None, "full-frame demand folds a tick's region away");

    // No frame rendered yet → panel size unknown → the spinner abstains (full repaint).
    let mut fresh = App::new_idle(AppState::new(POS.0, POS.1, 0.05));
    fresh.debug_start_nav(POS, POI, "Bench");
    let _ = fresh.take_dirty();
    fresh.advance_animations(InputClock(100));
    fresh.advance_animations(InputClock(200));
    let d = fresh.take_dirty();
    assert!(d.map, "the spinner still claims its repaint");
    assert_eq!(d.region, None, "…but abstains from a region before the first frame");
}

#[test]
fn overview_after_debug_plan_goes_quiet() {
    // The #500 bench flow: planning pushed over Home (debug_start_nav), answered with a
    // resolvable id → overview. The app must then go quiet: no repaint claims, no short wake.
    let mut app = App::new_idle(AppState::new(POS.0, POS.1, 0.05));
    // Isolate the "spinner leaves no repaint / wake behind" claim from the idle-return wake (which
    // the non-ride overview would otherwise legitimately arm).
    app.set_settings(Settings { idle_return: IdleReturn::Never, ..Settings::default() });
    nav_catalog(&mut app);
    app.debug_start_nav(POS, POI, "Bench");
    assert!(matches!(app.top_screen(), Screen::NavPlanning(_)));
    let _ = app.take_nav_request();
    let _ = app.take_dirty();
    app.notify_nav_result(Ok(7));
    assert!(matches!(app.top_screen(), Screen::RouteOverview(_)), "answer swaps to the overview");
    let _ = app.take_dirty();
    for i in 1..=20u32 {
        app.advance_animations(InputClock(1_000 + i * 107));
        let wake = app.ms_until_next_wake(1_000 + i * 107);
        assert!(
            !app.take_dirty().map && wake.is_none(),
            "pass {i}: overview must be quiet (dirty or wake {wake:?} claimed)"
        );
    }
}

#[test]
fn repeated_debug_requests_stack_one_planning_screen() {
    // The bench host repeats the `N` line against the flaky VCOM; only one planning screen may
    // result, or the host's answer strands the extras spinning forever (the #500 bench artifact).
    let mut app = App::new_idle(AppState::new(POS.0, POS.1, 0.05));
    app.set_settings(Settings { idle_return: IdleReturn::Never, ..Settings::default() });
    nav_catalog(&mut app);
    for _ in 0..3 {
        app.debug_start_nav(POS, POI, "Bench");
    }
    let planning = |app: &App| {
        // Count via the public seam: answering removes exactly the planning screens it finds.
        matches!(app.top_screen(), Screen::NavPlanning(_))
    };
    assert!(planning(&app));
    let _ = app.take_nav_request();
    app.notify_nav_result(Ok(7));
    assert!(matches!(app.top_screen(), Screen::RouteOverview(_)), "the answer lands on the one screen");
    let _ = app.take_dirty();
    app.advance_animations(InputClock(2_000));
    assert!(!app.take_dirty().map, "no stranded spinner keeps repainting");
    assert!(app.ms_until_next_wake(2_000).is_none(), "…or holds a short wake armed");
}

/// The shape-preview seam (#685 §4): a successful answer opens the computed overview
/// **preview-missing** — the host's cue to decimate and hand the ≤ 64-point copy in — and
/// `set_nav_preview` satisfies it (dirtying the frame). The cue never fires without a computed
/// overview on the stack, and a rider leaving the overview retires it.
#[test]
fn nav_preview_seam_fires_once_per_plan() {
    let bytes = fixture();
    let mut app = App::new_idle(AppState::new(POS.0, POS.1, 0.05));
    assert!(!app.nav_preview_missing(), "no computed overview, no cue");
    open_detail(&mut app, &bytes);
    let _req = request_route(&mut app);
    nav_catalog(&mut app);
    app.notify_nav_result(Ok(7));
    assert!(app.nav_preview_missing(), "the fresh overview wants its preview");

    let _ = app.take_dirty();
    app.set_nav_preview(&[(0, 0), (100, 100), (200, 150)]);
    assert!(!app.nav_preview_missing(), "fed once — the cue retires");
    assert!(app.take_dirty().map, "the handed-in preview repaints the overview");

    // Back off the overview: no computed overview up ⇒ no cue, even though the preview is stale.
    app.apply_gesture(Gesture::Back);
    assert!(!app.nav_preview_missing());
}

/// A re-plan clears the previous route's preview: the new overview starts preview-missing again,
/// so a stale shape can never draw under fresh bytes (the same id/index can carry a re-route).
#[test]
fn a_new_plan_starts_preview_less() {
    let bytes = fixture();
    let mut app = App::new_idle(AppState::new(POS.0, POS.1, 0.05));
    open_detail(&mut app, &bytes);
    let _req = request_route(&mut app);
    nav_catalog(&mut app);
    app.notify_nav_result(Ok(7));
    app.set_nav_preview(&[(0, 0), (1, 1)]);
    assert!(!app.nav_preview_missing());

    // Back to the detail, plan again (the reserved file is rewritten under the same id).
    app.apply_gesture(Gesture::Back);
    let _req = request_route(&mut app);
    app.notify_nav_result(Ok(7));
    assert!(app.nav_preview_missing(), "the re-plan's overview wants a fresh preview");
}
