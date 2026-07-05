//! Integration tests for the on-device POIs browser (#425): the Menu → category → list navigation,
//! the lazy static snapshot (populates from a `Reader` on the first draw, then stays frozen), the
//! empty-category and no-fix states, and that the reader-build seam ([`App::base_needs_reader`])
//! reports correctly for the POI list vs the frozen snapshot.
//!
//! Screens are driven through the real gesture path (`App::apply_gesture`), then a frame is rendered
//! with `App::render_frame` (which always passes `Some(reader)`, like the sim) so the snapshot fills.

use embedded_graphics::pixelcolor::Rgb888;
use obc_app::screen::Screen;
use obc_app::{App, AppState, Fix, Gesture};
use obc_reader::{rgb565_to_rgb888, MapCache, MapTables, Reader, SliceSource};
use obcm_testkit::{build_poi_map, PoiSpec};

mod common;
use common::Buf;

/// The fixture map bbox `(min_lon, min_lat, max_lon, max_lat)` — a 1°×1° square near 43° N, matching
/// the reader's POI query tests.
const BBOX: (i32, i32, i32, i32) = (7_000_000, 43_000_000, 8_000_000, 44_000_000);
/// The query point (lon, lat µdeg) all fixtures center on.
const POS: (i32, i32) = (7_500_000, 43_500_000);

/// A v6 map with Water and Campsite POIs near [`POS`], and an empty Accommodation category. Water
/// has three named points at increasing distance; Campsite has one unnamed point (so the fallback
/// label shows).
fn fixture() -> Vec<u8> {
    let water = vec![
        PoiSpec { lat: 43_500_500, lon: 7_500_000, subtype: 1, name: "Fountain North".into() }, // due north, nearest
        PoiSpec { lat: 43_500_000, lon: 7_501_000, subtype: 2, name: "Spring East".into() },    // due east
        PoiSpec { lat: 43_490_000, lon: 7_500_000, subtype: 1, name: "Well South".into() },     // due south, farther
    ];
    // Unnamed campsite (subtype 5 → "Campsite" fallback label).
    let campsite = vec![PoiSpec { lat: 43_501_000, lon: 7_500_000, subtype: 5, name: String::new() }];
    build_poi_map(BBOX, 512, &[(1, water), (2, campsite)])
}

/// Render one frame of `app` against the fixture map (always `Some(reader)`), returning the buffer.
fn render(app: &mut App, bytes: &[u8]) -> Buf {
    let cache = MapCache::new();
    let src = SliceSource(bytes);
    let tables = MapTables::parse(&src).expect("valid v6 file");
    let reader = Reader::new(&src, &tables, &cache);
    let mut buf = Buf::new(240, 320);
    app.render_frame(&mut buf, &reader, None, 240.0, 320.0, |c| {
        let (r, g, b) = rgb565_to_rgb888(c);
        Rgb888::new(r, g, b)
    });
    buf
}

/// Walk an idle `App` from Home into the POI list for `category`, leaving the list on top. Uses the
/// real gesture flow: Home `back-hold` → Menu, one clockwise detent to the POIs station, press →
/// category list, `steps` clockwise detents to the category, press → POI list.
fn open_poi_list(app: &mut App, steps: i32) {
    app.apply_gesture(Gesture::BackHold); // Home → Menu (compass)
    app.apply_gesture(Gesture::Turn(1)); // Routes(N) → POIs(E)
    app.apply_gesture(Gesture::Press); // → category list
    if steps != 0 {
        app.apply_gesture(Gesture::Turn(steps));
    }
    app.apply_gesture(Gesture::Press); // → POI list
}

/// The top screen is the POI list for the expected category — the whole navigation reached it.
#[test]
fn menu_to_category_to_list_navigation() {
    let mut app = App::new_idle(AppState::new(POS.0, POS.1, 0.05));
    // Home → Menu → POIs station → category list.
    app.apply_gesture(Gesture::BackHold);
    app.apply_gesture(Gesture::Turn(1));
    app.apply_gesture(Gesture::Press);
    assert!(matches!(app.top_screen(), Screen::PoiMenu(_)), "POIs opens the category list");

    // Category 0 (Water) → its POI list.
    app.apply_gesture(Gesture::Press);
    assert!(matches!(app.top_screen(), Screen::PoiList(_)), "picking a category opens its list");

    // Back walks list → category list → Menu.
    app.apply_gesture(Gesture::Back);
    assert!(matches!(app.top_screen(), Screen::PoiMenu(_)), "back returns to the category list");
    app.apply_gesture(Gesture::Back);
    assert!(matches!(app.top_screen(), Screen::Menu(_)), "back returns to the Menu");
}

/// The lazy snapshot populates from the `Reader` on the first draw with a fix, and the reader-build
/// seam reflects it: pending before the draw, satisfied after.
#[test]
fn lazy_snapshot_populates_on_first_draw() {
    let bytes = fixture();
    let mut app = App::new_idle(AppState::new(POS.0, POS.1, 0.05));
    app.state.user_fix = Some(Fix::at(POS.1, POS.0)); // (lat, lon) — a stationary fix at POS

    open_poi_list(&mut app, 0); // Water
                                // Before any draw, the POI list still needs the Reader (its query runs at draw).
    assert!(app.base_needs_reader(), "the POI list needs the Reader until it has snapshotted");

    let _ = render(&mut app, &bytes); // the first draw takes the snapshot
                                      // After the snapshot the seam reports the Reader is no longer needed (frozen list draws alone).
    assert!(!app.base_needs_reader(), "once snapshotted the POI list draws without the Reader");
    assert_eq!(app.poi_snapshot_len(), 3, "all three Water POIs snapshotted (nearest-16, only 3 exist)");
}

/// The snapshot is static: a fix that moves after the first draw does not change the frozen list.
#[test]
fn snapshot_is_static_after_first_draw() {
    let bytes = fixture();
    let mut app = App::new_idle(AppState::new(POS.0, POS.1, 0.05));
    app.state.user_fix = Some(Fix::at(POS.1, POS.0));
    open_poi_list(&mut app, 0);
    let _ = render(&mut app, &bytes);
    let n0 = app.poi_snapshot_len();
    assert_eq!(n0, 3);

    // Move far away and re-render several times — the snapshot must not re-query.
    app.state.user_fix = Some(Fix::at(43_900_000, 7_900_000));
    for _ in 0..3 {
        let _ = render(&mut app, &bytes);
    }
    assert_eq!(app.poi_snapshot_len(), n0, "the list is frozen — a moved fix doesn't re-query");
}

/// Re-entering a category takes a fresh snapshot (opening a POI list invalidates the scratch).
#[test]
fn reentering_requeries() {
    let bytes = fixture();
    let mut app = App::new_idle(AppState::new(POS.0, POS.1, 0.05));
    app.state.user_fix = Some(Fix::at(POS.1, POS.0));

    open_poi_list(&mut app, 0); // Water
    let _ = render(&mut app, &bytes);
    assert_eq!(app.poi_snapshot_len(), 3);

    app.apply_gesture(Gesture::Back); // back to category list
                                      // Opening the *next* category must re-query (a different set), proving the scratch invalidates.
    app.apply_gesture(Gesture::Turn(1)); // Water → Campsite
    app.apply_gesture(Gesture::Press);
    assert!(app.base_needs_reader(), "re-entering a category needs the Reader again (scratch invalidated)");
    let _ = render(&mut app, &bytes);
    assert_eq!(app.poi_snapshot_len(), 1, "Campsite has one POI — the fresh snapshot replaced Water's");
}

/// An empty category snapshots to zero POIs; the "No POIs in this map" empty state draws (parchment
/// body, no rows).
#[test]
fn empty_category_snapshots_empty() {
    let bytes = fixture();
    let mut app = App::new_idle(AppState::new(POS.0, POS.1, 0.05));
    app.state.user_fix = Some(Fix::at(POS.1, POS.0));

    // Accommodation is category index 2 (Water, Campsite, Accommodation…) and empty in the fixture.
    open_poi_list(&mut app, 2);
    let _ = render(&mut app, &bytes);
    assert!(!app.base_needs_reader(), "an empty category still counts as snapshotted (query ran)");
    assert_eq!(app.poi_snapshot_len(), 0, "the empty category snapshots to zero POIs");
}

/// With no fix ever, the query can't run: the list stays un-snapshotted and shows the "No position"
/// state, and the seam keeps asking for the Reader (so a fix arriving later still snapshots).
#[test]
fn no_fix_shows_no_position_and_keeps_needing_reader() {
    let bytes = fixture();
    let mut app = App::new_idle(AppState::new(POS.0, POS.1, 0.05));
    // No fix set at all.
    open_poi_list(&mut app, 0);
    let _ = render(&mut app, &bytes);
    assert!(app.base_needs_reader(), "with no fix the snapshot can't be taken — still needs the Reader");
    assert_eq!(app.poi_snapshot_len(), 0, "no snapshot without a fix");

    // A fix arrives; the next draw finally snapshots.
    app.state.user_fix = Some(Fix::at(POS.1, POS.0));
    let _ = render(&mut app, &bytes);
    assert!(!app.base_needs_reader(), "the fix let the snapshot land");
    assert_eq!(app.poi_snapshot_len(), 3);
}
