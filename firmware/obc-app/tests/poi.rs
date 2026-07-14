//! Integration tests for the on-device POIs browser (#425): the Menu → category → list navigation,
//! the lazy static snapshot (populates from a `Reader` in the pre-draw `prepare` pass — #803 moved
//! it out of `draw` — then stays frozen), the empty-category and no-fix states, and that the
//! reader-build seam ([`App::base_needs_reader`]) reports correctly for the POI list vs the frozen
//! snapshot.
//!
//! Screens are driven through the real gesture path (`App::apply_gesture`), then a frame is rendered
//! with `App::render_frame` (which always passes `Some(reader)`, like the sim); its pre-draw
//! `prepare` pass fills the snapshot, and `draw` then consumes it read-only.

use embedded_graphics::pixelcolor::Rgb888;
use obc_app::screen::Screen;
use obc_app::{App, AppState, Fix, Gesture};
use obc_reader::{rgb565_to_rgb888, MapCache, MapTables, Reader, SliceSource, POI_HOURS_BLOB_LEN};
use obcm_testkit::{build_poi_map, build_poi_map_with_hours, PoiSpec};

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
        PoiSpec { lat: 43_500_500, lon: 7_500_000, subtype: 1, name: "Fountain North".into(), hours_ref: 0xFFFF }, // due north, nearest
        PoiSpec { lat: 43_500_000, lon: 7_501_000, subtype: 2, name: "Spring East".into(), hours_ref: 0xFFFF }, // due east
        PoiSpec { lat: 43_490_000, lon: 7_500_000, subtype: 1, name: "Well South".into(), hours_ref: 0xFFFF }, // due south, farther
    ];
    // Unnamed campsite (subtype 5 → "Campsite" fallback label).
    let campsite =
        vec![PoiSpec { lat: 43_501_000, lon: 7_500_000, subtype: 5, name: String::new(), hours_ref: 0xFFFF }];
    build_poi_map(BBOX, 512, &[(1, water), (2, campsite)])
}

/// Render one frame of `app` against the fixture map (always `Some(reader)`), returning the buffer.
fn render(app: &mut App, bytes: &[u8]) -> Buf {
    let cache = MapCache::new();
    let src = SliceSource(bytes);
    let tables = MapTables::parse(&src).expect("valid v7 file");
    let reader = Reader::new(&src, &tables, &cache);
    let mut buf = Buf::new(240, 320);
    app.render_frame(&mut buf, &reader, None, 240.0, 320.0, |c| {
        let (r, g, b) = rgb565_to_rgb888(c);
        Rgb888::new(r, g, b)
    });
    buf
}

/// Walk an idle `App` from Home into the POI list for `category`, leaving the list on top. Uses the
/// real gesture flow: Home `back-hold` → Menu, two clockwise detents to the POIs station (the menu
/// order is Routes · Rides · POIs · Map · Settings), press → category list, `steps` clockwise detents
/// to the category, press → POI list.
fn open_poi_list(app: &mut App, steps: i32) {
    app.apply_gesture(Gesture::BackHold); // Home → Menu (compass)
    app.apply_gesture(Gesture::Turn(2)); // Routes → Rides → POIs
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
    // Home → Menu → POIs station (two detents past Routes) → category list.
    app.apply_gesture(Gesture::BackHold);
    app.apply_gesture(Gesture::Turn(2));
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

/// A 29-byte hours-pool blob from `flags` + per-day `(open_q, close_q)` slot pairs (Mon..Sun) — the
/// shared shape the reader/packer hours tests build.
fn blob(flags: u8, days: [[(u8, u8); 2]; 7]) -> [u8; POI_HOURS_BLOB_LEN] {
    let mut b = [0u8; POI_HOURS_BLOB_LEN];
    b[0] = flags;
    let mut i = 1;
    for day in &days {
        for &(o, c) in day {
            b[i] = o;
            b[i + 1] = c;
            i += 2;
        }
    }
    b
}

/// A Water category with two POIs referencing a two-blob hours pool: one Mon–Sun 08:00–18:00 shop
/// (ref 0) and one with no hours (ref 0xFFFF), so the detail tests cover both the "has hours" and
/// "hours not listed" branches from a real file layout.
fn hours_fixture() -> Vec<u8> {
    // Blob 0: open every day 08:00-18:00 (quarter-hours 32..72).
    let all_week = blob(0, [[(32, 72), (0, 0)]; 7]);
    let water = vec![
        PoiSpec { lat: 43_500_500, lon: 7_500_000, subtype: 1, name: "Shop North".into(), hours_ref: 0 }, // nearest, has hours
        PoiSpec { lat: 43_490_000, lon: 7_500_000, subtype: 2, name: "Well South".into(), hours_ref: 0xFFFF }, // farther, no hours
    ];
    build_poi_map_with_hours(BBOX, 512, &[(1, water)], &[all_week])
}

/// Pressing a POI in the list opens the detail screen; back returns to the list.
#[test]
fn press_opens_detail_and_back_returns() {
    let bytes = hours_fixture();
    let mut app = App::new_idle(AppState::new(POS.0, POS.1, 0.05));
    app.state.user_fix = Some(Fix::at(POS.1, POS.0));

    open_poi_list(&mut app, 0); // Water
    let _ = render(&mut app, &bytes); // first draw takes the snapshot (Press needs it)
    assert_eq!(app.poi_snapshot_len(), 2, "two Water POIs snapshotted");

    app.apply_gesture(Gesture::Press);
    assert!(matches!(app.top_screen(), Screen::PoiDetail(_)), "pressing a POI opens the detail");

    app.apply_gesture(Gesture::Back);
    assert!(matches!(app.top_screen(), Screen::PoiList(_)), "back returns to the POI list");
}

/// Pressing before any snapshot exists (no draw yet) is a no-op — nothing to open.
#[test]
fn press_without_snapshot_is_noop() {
    let bytes = hours_fixture();
    let mut app = App::new_idle(AppState::new(POS.0, POS.1, 0.05));
    app.state.user_fix = Some(Fix::at(POS.1, POS.0));
    open_poi_list(&mut app, 0);
    // No render → no snapshot yet.
    app.apply_gesture(Gesture::Press);
    assert!(matches!(app.top_screen(), Screen::PoiList(_)), "no snapshot ⇒ press stays on the list");
    let _ = bytes;
}

/// The detail screen resolves its hours on the first draw with a `Reader`, and the reader-build seam
/// reflects it: pending before the draw, satisfied after (so the board host stops rebuilding).
#[test]
fn detail_resolves_hours_on_first_draw() {
    let bytes = hours_fixture();
    let mut app = App::new_idle(AppState::new(POS.0, POS.1, 0.05));
    app.state.user_fix = Some(Fix::at(POS.1, POS.0));

    open_poi_list(&mut app, 0);
    let _ = render(&mut app, &bytes); // snapshot
    app.apply_gesture(Gesture::Press); // open the detail for the nearest (Shop North, ref 0)
    assert!(matches!(app.top_screen(), Screen::PoiDetail(_)));

    // Before the detail's first draw its hours read is pending — the seam keeps the Reader built.
    assert!(app.base_needs_reader(), "the detail needs the Reader until it resolves its hours");
    let _ = render(&mut app, &bytes); // the first detail draw resolves the schedule
    assert!(!app.base_needs_reader(), "once resolved the detail draws without the Reader");
    // A few more frames must not re-ask (the cache is sticky).
    for _ in 0..3 {
        let _ = render(&mut app, &bytes);
    }
    assert!(!app.base_needs_reader(), "the resolved schedule cache stays put");
}

/// A POI with `hours_ref` 0xFFFF resolves to *no hours* on the first draw too — the seam stops
/// asking for the Reader even though there's nothing to show ("Hours not listed").
#[test]
fn detail_no_hours_still_resolves_once() {
    let bytes = hours_fixture();
    let mut app = App::new_idle(AppState::new(POS.0, POS.1, 0.05));
    app.state.user_fix = Some(Fix::at(POS.1, POS.0));

    open_poi_list(&mut app, 0);
    let _ = render(&mut app, &bytes); // snapshot (nearest is Shop North; turn to the no-hours one)
    app.apply_gesture(Gesture::Turn(1)); // highlight Well South (ref 0xFFFF)
    app.apply_gesture(Gesture::Press);
    assert!(matches!(app.top_screen(), Screen::PoiDetail(_)));

    assert!(app.base_needs_reader(), "the no-hours detail still needs one Reader frame to resolve");
    let _ = render(&mut app, &bytes);
    assert!(!app.base_needs_reader(), "a no-hours POI resolves (to None) once, then draws alone");
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
