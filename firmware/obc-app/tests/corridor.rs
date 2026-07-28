//! Integration tests for the App-owned **route-corridor snapshot** (epic #946, U2): the frozen
//! snapshot's take/freeze/re-take semantics and the **host reader seam** — that
//! [`App::base_needs_reader`] asks the board to build the streamed-map `Reader` exactly until the
//! one query lands, then stops.
//!
//! No screen exists yet (U3 draws the list), so the request is armed through the App façade
//! ([`App::arm_corridor`]) the way U3's screen entry will. Frames go through the real
//! [`App::render_frame`] path, whose pre-draw `prepare` boundary is where the query runs.

use embedded_graphics::pixelcolor::Rgb888;
use obc_app::{App, AppState};
use obc_formats::io::{ByteSink, SliceSource};
use obc_reader::{rgb565_to_rgb888, MapCache, MapTables, PoiCategory, PoiCategorySet, Reader};
use obc_route::{RouteIndex, RouteReader};
use obcm_testkit::{build_poi_map, PoiSpec};

mod common;
use common::Buf;

/// The map bbox the POI fixture packs into, and the packer's default POI chunk size.
const BBOX: (i32, i32, i32, i32) = (7_000_000, 47_000_000, 9_000_000, 49_000_000);
const CS: usize = 512;

/// A `ByteSink` over a growable `Vec` — the host's "write the file to RAM" backing.
#[derive(Default)]
struct VecSink(Vec<u8>);

impl ByteSink for VecSink {
    fn write(&mut self, b: &[u8]) -> Result<(), obc_formats::io::Error> {
        self.0.extend_from_slice(b);
        Ok(())
    }
    fn patch_at(&mut self, off: u32, b: &[u8]) -> Result<(), obc_formats::io::Error> {
        let o = off as usize;
        self.0[o..o + b.len()].copy_from_slice(b);
        Ok(())
    }
}

/// A due-east 30-point route along 48.0000° N from 7.8000° E, converted to `.obcr`.
fn route_bytes() -> Vec<u8> {
    let mut gpx = String::from(r#"<?xml version="1.0"?><gpx version="1.1"><trk><trkseg>"#);
    for i in 0..30 {
        let lon = 7.8000 + 0.0020 * i as f64;
        gpx.push_str(&format!(r#"<trkpt lat="48.0000" lon="{lon:.4}"><ele>200.0</ele></trkpt>"#));
    }
    gpx.push_str("</trkseg></trk></gpx>");
    let mut sink = VecSink::default();
    obc_route::gpx_to_obcr(&SliceSource(gpx.as_bytes()), "East", &mut sink).unwrap();
    sink.0
}

/// Three Water POIs beside the route (all inside the 300 m corridor) plus one bike shop, so a
/// filter change genuinely changes the answer.
fn map_bytes() -> Vec<u8> {
    let water = vec![
        PoiSpec { lat: 48_001_000, lon: 7_810_000, subtype: 1, name: "W1".into(), hours_ref: 0xFFFF },
        PoiSpec { lat: 47_999_000, lon: 7_830_000, subtype: 1, name: "W2".into(), hours_ref: 0xFFFF },
        PoiSpec { lat: 48_000_500, lon: 7_850_000, subtype: 1, name: "W3".into(), hours_ref: 0xFFFF },
    ];
    let shops = vec![PoiSpec { lat: 48_000_500, lon: 7_820_000, subtype: 18, name: "S1".into(), hours_ref: 0xFFFF }];
    build_poi_map(BBOX, CS, &[(1, water), (6, shops)])
}

/// Render one frame with both the map `Reader` and the streamed route — the frame shape the board
/// produces when [`App::base_needs_reader`] says it must.
fn render_with_route(app: &mut App, map: &[u8], obcr: &[u8]) {
    let cache = MapCache::new();
    let map_src = SliceSource(map);
    let tables = MapTables::parse(&map_src).expect("valid .obcm");
    let reader = Reader::new(&map_src, &tables, &cache);

    let route_src = SliceSource(obcr);
    let idx = RouteIndex::read(&route_src).expect("valid .obcr");
    let route = RouteReader::new(&idx, &route_src);

    let mut buf = Buf::new(240, 320);
    app.render_frame(&mut buf, &reader, Some(&route), 240.0, 320.0, |c| {
        let (r, g, b) = rgb565_to_rgb888(c);
        Rgb888::new(r, g, b)
    });
}

/// Render one frame with the map `Reader` but **no** route — the frame the host produces before a
/// route is opened. The corridor query has nothing to project onto and must simply retry later.
fn render_without_route(app: &mut App, map: &[u8]) {
    let cache = MapCache::new();
    let map_src = SliceSource(map);
    let tables = MapTables::parse(&map_src).expect("valid .obcm");
    let reader = Reader::new(&map_src, &tables, &cache);
    let mut buf = Buf::new(240, 320);
    app.render_frame(&mut buf, &reader, None, 240.0, 320.0, |c| {
        let (r, g, b) = rgb565_to_rgb888(c);
        Rgb888::new(r, g, b)
    });
}

/// A `ByteSource` whose every streamed read fails — a card that has stopped answering, or a POI
/// section that can't be walked. `len` still reports the real file, so the reader's bounds checks
/// pass and the failure lands where a real one would: in the quadtree/chunk reads.
struct FailingSource<'a>(SliceSource<'a>);

impl obc_formats::io::ByteSource for FailingSource<'_> {
    fn read_at(&self, _off: u32, _buf: &mut [u8]) -> Result<(), obc_formats::io::Error> {
        Err(obc_formats::io::Error::Io)
    }
    fn len(&self) -> u32 {
        self.0.len()
    }
}

/// Render one frame whose map `Reader` streams from a **failing** source. The tables are parsed off
/// a clean source first (the host parses once per map load, not per frame), so the failure is
/// exactly the one that matters here: the corridor query's own reads.
fn render_with_failing_map(app: &mut App, map: &[u8], obcr: &[u8]) {
    let clean = SliceSource(map);
    let tables = MapTables::parse(&clean).expect("valid .obcm");
    let failing = FailingSource(SliceSource(map));
    let cache = MapCache::new();
    let reader = Reader::new(&failing, &tables, &cache);

    let route_src = SliceSource(obcr);
    let idx = RouteIndex::read(&route_src).expect("valid .obcr");
    let route = RouteReader::new(&idx, &route_src);

    let mut buf = Buf::new(240, 320);
    app.render_frame(&mut buf, &reader, Some(&route), 240.0, 320.0, |c| {
        let (r, g, b) = rgb565_to_rgb888(c);
        Rgb888::new(r, g, b)
    });
}

/// **The host seam.** An idle App asks for no `Reader`; arming a corridor request makes it ask;
/// the frame that takes the snapshot satisfies it; every frame after that is quiet again.
#[test]
fn the_reader_is_built_only_until_the_snapshot_lands() {
    let (map, obcr) = (map_bytes(), route_bytes());
    let mut app = App::new_idle(AppState::new(7_800_000, 48_000_000, 0.05));
    assert!(!app.base_needs_reader(), "an idle Home frame needs no Reader");

    app.arm_corridor(PoiCategorySet::ALL, 0);
    assert!(app.base_needs_reader(), "an armed corridor keeps the Reader built");
    assert!(app.corridor_snapshot_pending());

    render_with_route(&mut app, &map, &obcr);
    assert!(!app.corridor_snapshot_pending(), "the snapshot landed on the first eligible frame");
    assert!(!app.base_needs_reader(), "…and the host stops building the Reader");
    assert_eq!(app.corridor_snapshot_len(), 4, "three water points and one bike shop are in the corridor");

    // Repeated frames must not re-query — the seam stays quiet.
    render_with_route(&mut app, &map, &obcr);
    assert!(!app.base_needs_reader());
    assert_eq!(app.corridor_snapshot_len(), 4);
}

/// A frame without a route can't take the snapshot: the seam keeps asking (it retries), and the
/// first frame that does carry a route settles it. This is the "no route loaded yet" boot order.
#[test]
fn a_frame_without_a_route_retries_rather_than_settling() {
    let (map, obcr) = (map_bytes(), route_bytes());
    let mut app = App::new_idle(AppState::new(7_800_000, 48_000_000, 0.05));
    app.arm_corridor(PoiCategorySet::ALL, 0);

    render_without_route(&mut app, &map);
    assert!(app.corridor_snapshot_pending(), "no route ⇒ nothing to project onto");
    assert!(app.base_needs_reader(), "so the seam keeps asking");
    assert_eq!(app.corridor_snapshot_len(), 0);

    render_with_route(&mut app, &map, &obcr);
    assert!(!app.base_needs_reader(), "the first frame with a route settles it");
    assert_eq!(app.corridor_snapshot_len(), 4);
}

/// **A failing query settles, it does not retry.** A corrupt POI section or a card that has stopped
/// answering is the one case where retrying would be worst: the query's most expensive form would
/// re-run on **every** rendered frame with the `Reader` kept built — exactly the per-frame SD work
/// the #115/#425 discipline forbids. So an errored take settles on an empty list, like
/// `PoiScratch` does, and only an explicit re-entry retries.
#[test]
fn an_erroring_source_settles_after_one_attempt() {
    let (map, obcr) = (map_bytes(), route_bytes());
    let mut app = App::new_idle(AppState::new(7_800_000, 48_000_000, 0.05));
    app.arm_corridor(PoiCategorySet::ALL, 0);
    assert!(app.base_needs_reader());

    render_with_failing_map(&mut app, &map, &obcr);
    assert!(!app.corridor_snapshot_pending(), "one attempt is all a failing source gets");
    assert!(!app.base_needs_reader(), "the seam goes quiet — no per-frame retry of the worst case");
    assert_eq!(app.corridor_snapshot_len(), 0, "settled empty, never a half-filled list");

    // And it stays quiet however many frames go by.
    for _ in 0..3 {
        render_with_failing_map(&mut app, &map, &obcr);
    }
    assert!(!app.base_needs_reader());

    // Re-entry is the retry, and once the source works the snapshot lands normally.
    app.invalidate_corridor();
    assert!(app.base_needs_reader(), "re-entry retries the identical key");
    render_with_route(&mut app, &map, &obcr);
    assert_eq!(app.corridor_snapshot_len(), 4, "a healthy frame fills it");
    assert!(!app.base_needs_reader());
}

/// **The frozen contract (#115).** Once taken, the snapshot does not move: neither more frames nor
/// a fix that has ridden on re-query it. Only an explicit re-arm (a filter change) or an
/// `invalidate` (screen re-entry) takes a fresh one.
#[test]
fn the_snapshot_is_frozen_until_refiltered_or_invalidated() {
    let (map, obcr) = (map_bytes(), route_bytes());
    let mut app = App::new_idle(AppState::new(7_800_000, 48_000_000, 0.05));
    app.arm_corridor(PoiCategorySet::ALL, 0);
    render_with_route(&mut app, &map, &obcr);
    let frozen: Vec<u32> = app.corridor_snapshot().iter().map(|c| c.dist_along_m).collect();
    assert_eq!(frozen.len(), 4);

    for _ in 0..3 {
        render_with_route(&mut app, &map, &obcr);
    }
    let after: Vec<u32> = app.corridor_snapshot().iter().map(|c| c.dist_along_m).collect();
    assert_eq!(after, frozen, "membership, order and distances are frozen on take");

    // A filter change re-arms: the stale rows are dropped at once, and the seam asks again.
    app.arm_corridor(PoiCategorySet::only(PoiCategory::Water), 0);
    assert!(app.base_needs_reader(), "a filter change re-queries");
    assert_eq!(app.corridor_snapshot_len(), 0, "and drops the stale rows immediately");
    render_with_route(&mut app, &map, &obcr);
    assert_eq!(app.corridor_snapshot_len(), 3, "the Water-only list");

    // Re-entry (`invalidate`) re-takes the identical key.
    app.invalidate_corridor();
    assert!(app.base_needs_reader());
    render_with_route(&mut app, &map, &obcr);
    assert_eq!(app.corridor_snapshot_len(), 3);

    // Closing the screen stops the request entirely.
    app.clear_corridor();
    assert!(!app.base_needs_reader());
    assert_eq!(app.corridor_snapshot_len(), 0);
}

/// The **progress anchor** is part of the key: arming at a later anchor re-queries and drops what
/// the rider has already passed, while the rows stay ordered by along-route distance and carry the
/// distance still to go.
#[test]
fn the_progress_anchor_windows_the_snapshot() {
    let (map, obcr) = (map_bytes(), route_bytes());
    let mut app = App::new_idle(AppState::new(7_800_000, 48_000_000, 0.05));
    app.arm_corridor(PoiCategorySet::ALL, 0);
    render_with_route(&mut app, &map, &obcr);
    let first = app.corridor_snapshot()[0].dist_along_m;
    assert!(app.corridor_snapshot().windows(2).all(|w| w[0].dist_along_m <= w[1].dist_along_m));

    let anchor = first + 1;
    app.arm_corridor(PoiCategorySet::ALL, anchor);
    assert!(app.base_needs_reader(), "a new anchor is a new key");
    render_with_route(&mut app, &map, &obcr);
    assert_eq!(app.corridor_snapshot_len(), 3, "the passed entry is gone");
    for c in app.corridor_snapshot() {
        assert!(c.dist_along_m >= anchor, "only what is still ahead");
        assert_eq!(c.poi.distance_m, c.dist_along_m - anchor, "the row's distance-to-go");
    }
}
