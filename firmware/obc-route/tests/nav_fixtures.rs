#![cfg(feature = "external-fixtures")]

mod common;
use common::nav::{digest, plan_p};
use common::{route_points, VecSink};
use obc_formats::io::SliceSource;
use obc_reader::{MapCache, MapTables, NavTileCache, Reader};
use obc_route::nav::{plan_route, NavScratch};

/// Over the real packed grimsel map, the same endpoints planned under Road (profile 0) and MTB
/// (profile 2) give different polylines: the profile weights steer the search end to end. The raw
/// lengths differ by ~2.8 km, so the assert cannot pass on emit jitter.
///
/// The endpoints are pinned from a deterministic sweep of the map's own nav nodes, chosen inside
/// the canonical grimsel extract bbox (`8.15034,46.48261,8.46007,46.72070`). The header bbox is
/// always wider than the extract, so pinning against the extract bbox is what survives a re-pack.
/// A re-pack from a newer OSM snapshot can still move the graph enough to need a re-pin.
// Reads the fixture from disk, which Miri's default isolation forbids, so it is skipped there. The
// UB tripwire is the record decode over the synthetic fixtures, which stay in the Miri run.
#[cfg_attr(miri, ignore)]
#[test]
fn road_vs_mtb_diverge_over_grimsel() {
    let bytes = obc_fixtures::read("sim-grimsel", "grimsel.obcm");
    let from = (8_169_610, 46_694_536);
    let to = (8_217_309, 46_706_261);

    let (road, obcr_road, _) = plan_p(&bytes, from, to, "Road", 0);
    let (mtb, obcr_mtb, _) = plan_p(&bytes, from, to, "MTB", 2);
    let road = road.expect("Road plans");
    let mtb = mtb.expect("MTB plans");

    let pts_road = route_points(&obcr_road);
    let pts_mtb = route_points(&obcr_mtb);
    assert_ne!(pts_road, pts_mtb, "Road vs MTB must pick different polylines here");
    assert_ne!(road.total_distance_m, mtb.total_distance_m, "the two profiles' picks differ in raw ground length too");
}

/// The Grimsel map's nav graph planned through the Grimsel terrain sidecar, the same file the
/// simulator mounts. Nothing synthetic: this is the number a rider would see on the Route overview.
// Reads the committed fixtures from disk, so Miri skips it.
#[cfg_attr(miri, ignore)]
#[test]
fn a_real_grimsel_plan_carries_the_pass_road_profile() {
    let map = obc_fixtures::read("sim-grimsel", "grimsel.obcm");
    let dem = obc_fixtures::read("sim-grimsel", "grimsel.obcd");
    let terrain_src = SliceSource(&dem);
    let mut terrain = obc_elevation::TerrainElevation::<{ obc_elevation::DEFAULT_TILE_SLOTS }>::parse(&terrain_src)
        .expect("the baked terrain parses");

    // Innertkirchen → up the pass road (the profile-divergence fixture's endpoints).
    let (from, to) = ((8_169_610, 46_694_536), (8_217_309, 46_706_261));
    let src = SliceSource(&map);
    let tables = MapTables::parse(&src).expect("grimsel parses");
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let mut scratch = Box::new(NavScratch::<{ obc_route::NAV_MAX_NODES }>::new());
    let mut tiles = NavTileCache::new();
    let mut sink = VecSink::default();
    let route = plan_route(&r, from, to, "Grimsel", 0, &mut scratch, &mut tiles, &mut terrain, &mut sink)
        .expect("the pass road plans");

    // Alpine ground, never the 0 m a missing fill would leave, and a real but not absurd climb.
    assert!((500..=2_200).contains(&route.min_ele_m), "min {} m is not alpine ground", route.min_ele_m);
    assert!((500..=2_600).contains(&route.max_ele_m), "max {} m is not alpine ground", route.max_ele_m);
    assert!(route.max_ele_m > route.min_ele_m + 100, "a pass road is not flat ({route:?})");
    assert!((100..=3_000).contains(&route.total_ascent_m), "ascent {} m is implausible", route.total_ascent_m);
    let pts = route_points(&sink.buf);
    assert!(pts.iter().all(|p| p.ele > 0), "every stored point has a real height");
    let (hits, misses) = terrain.stats();
    assert!(hits > misses, "the 4-tile cache serves the walk ({hits} hit / {misses} miss)");
}

/// Round-trip parity, the property the shared dead-band exists for: write a planned route out as
/// GPX and re-import it through [`gpx_to_obcr`](obc_route::gpx_to_obcr). The re-imported climb must
/// agree with the header the planner wrote. Without the emit-time fill both sides are 0 and the
/// check is vacuous. This route lies wholly inside the terrain coverage, so every exported point
/// has a measured height and the export cannot turn an unknown span into a false sample.
#[cfg_attr(miri, ignore)] // reads the committed fixtures from disk — see the note above
#[test]
fn a_planned_route_exported_to_gpx_and_reimported_keeps_its_climb() {
    let map = obc_fixtures::read("sim-grimsel", "grimsel.obcm");
    let dem = obc_fixtures::read("sim-grimsel", "grimsel.obcd");
    let terrain_src = SliceSource(&dem);
    let mut terrain =
        obc_elevation::TerrainElevation::<{ obc_elevation::DEFAULT_TILE_SLOTS }>::parse(&terrain_src).unwrap();

    let (from, to) = ((8_169_610, 46_694_536), (8_217_309, 46_706_261));
    let src = SliceSource(&map);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let mut scratch = Box::new(NavScratch::<{ obc_route::NAV_MAX_NODES }>::new());
    let mut tiles = NavTileCache::new();
    let mut sink = VecSink::default();
    let planned =
        plan_route(&r, from, to, "Grimsel", 0, &mut scratch, &mut tiles, &mut terrain, &mut sink).expect("plans");

    // The export: one `<trkpt>` per stored point, exactly the fields an exporter has to hand.
    let mut gpx = String::from("<gpx><trk><trkseg>");
    for p in route_points(&sink.buf) {
        assert!(p.ele > 0, "an exported point with no height would export a lie");
        gpx.push_str(&format!(
            "<trkpt lat=\"{:.6}\" lon=\"{:.6}\"><ele>{}</ele></trkpt>",
            p.lat as f64 / 1e6,
            p.lon as f64 / 1e6,
            p.ele
        ));
    }
    gpx.push_str("</trkseg></trk></gpx>");

    let mut back = VecSink::default();
    let reimported = obc_route::gpx_to_obcr(&SliceSource(gpx.as_bytes()), "Grimsel", &mut back).expect("re-imports");

    assert_eq!((reimported.min_ele_m, reimported.max_ele_m), (planned.min_ele_m, planned.max_ele_m));
    let (a, b) = (planned.total_ascent_m as i64, reimported.total_ascent_m as i64);
    assert!(
        (a - b).abs() * 20 <= a.max(1),
        "planned +{a} m vs re-imported +{b} m — the two dead-band integrations must agree within 5%"
    );
}

/// Stock profiles on the registered Grimsel map produce stable OBCR bytes.
#[cfg_attr(miri, ignore)]
#[test]
fn the_registered_grimsel_fixture_routes_byte_identically_on_every_profile() {
    let bytes = obc_fixtures::read("sim-grimsel", "grimsel.obcm");
    let (from, to) = ((8_169_610, 46_694_536), (8_217_309, 46_706_261));
    let actual = core::array::from_fn::<_, 4, _>(|idx| {
        let (res, obcr, _) = plan_p(&bytes, from, to, "Grimsel", idx as u8);
        res.unwrap_or_else(|e| panic!("profile {idx} plans on grimsel, got {e:?}"));
        digest(&obcr)
    });
    assert_eq!(actual, GRIMSEL_ROUTE_DIGESTS, "registered fixture routes moved");
}

const GRIMSEL_ROUTE_DIGESTS: [u64; 4] =
    [0xf19d_4c75_e881_a991, 0xb5ae_0e8d_eb90_d7b7, 0xbf1c_9f49_2699_20e0, 0x618f_00e9_7a9c_7042];
