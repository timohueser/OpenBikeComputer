//! The POI create-route flow (epic #116, R4): the detail's press → "Create a route?" confirm, the
//! one-shot [`NavRequest`] seam the host drains ([`App::take_nav_request`]), and the host's answer
//! ([`App::notify_nav_result`]) — success swaps the confirm for the computed-route overview
//! (activated, length only), the locked two failure tiers swap it for the failure card, and the
//! overview's accept honours the ride state (idle → start; tracking → the existing save/swap
//! prompt). Screens are driven through the real gesture path with the POI harness's fixture map,
//! exactly like `poi.rs`.

use embedded_graphics::pixelcolor::Rgb888;
use obc_app::screen::Screen;
use obc_app::{App, AppState, Fix, Gesture, Mode, NavRequest, RouteSummary};
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

/// Render one frame against the fixture (fills the lazy POI snapshot, like the sim's draw).
fn render(app: &mut App, bytes: &[u8]) {
    let cache = MapCache::new();
    let src = SliceSource(bytes);
    let tables = MapTables::parse(&src).expect("valid fixture");
    let reader = Reader::new(&src, &tables, &cache);
    let mut buf = Buf::new(240, 320);
    app.render_frame(&mut buf, &reader, None, 240.0, 320.0, |c| {
        let (r, g, b) = rgb565_to_rgb888(c);
        Rgb888::new(r, g, b)
    });
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
fn request_route(app: &mut App) -> NavRequest {
    app.apply_gesture(Gesture::Press); // detail → confirm
    assert!(matches!(app.top_screen(), Screen::NavConfirm(_)), "detail press opens the confirm");
    app.apply_gesture(Gesture::Press); // Create route (row 0)
    assert!(matches!(app.top_screen(), Screen::NavConfirm(_)), "the confirm stays up while the host plans");
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
    app.apply_gesture(Gesture::Press); // Home → Route menu
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
fn result_without_a_confirm_on_stack_is_dropped() {
    let mut app = App::new_idle(AppState::new(POS.0, POS.1, 0.05));
    nav_catalog(&mut app);
    let _ = app.take_dirty();
    app.notify_nav_result(Ok(7));
    assert!(matches!(app.top_screen(), Screen::Home(_)), "no confirm up ⇒ the answer is dropped");
    assert_eq!(app.activity.active_route, None, "…and nothing activates behind the rider's back");
    assert!(!app.take_dirty().map, "…and nothing repaints");
}
