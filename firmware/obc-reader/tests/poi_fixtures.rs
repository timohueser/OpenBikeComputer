#![cfg(feature = "external-fixtures")]
use obc_reader::{MapCache, MapTables, Poi, PoiCategory, Reader, SliceSource, MAX_POI_RESULTS};

// === Real-data smoke test ====================================================

/// Load the committed real Monaco map and query a populated category near a Monaco coordinate:
/// ≤ 16 ascending results with plausible distances. Monaco's Water category has 28 POIs, so a
/// nearest-16 query fills and the results are the closest 16.
#[test]
fn monaco_water_query_smoke() {
    let bytes = obc_fixtures::read("sim-monaco", "monaco.obcm");
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);

    // A point in central Monaco (lon, lat µdeg).
    let pos = (7_420_000, 43_730_000);
    let mut out = heapless::Vec::<Poi, MAX_POI_RESULTS>::new();
    r.nearest_pois(PoiCategory::Water, pos, &mut out).unwrap();

    assert_eq!(out.len(), MAX_POI_RESULTS, "Monaco has > 16 water POIs; the query fills");
    // Ascending, all within a few km of the query (Monaco is tiny), every subtype in the Water range.
    let mut prev = 0u32;
    for p in &out {
        assert!(p.distance_m >= prev, "ascending distances");
        prev = p.distance_m;
        assert!(p.distance_m < 5_000, "a nearest-16 water POI in Monaco is within a few km");
        assert!(matches!(p.subtype, 1..=4), "Water subtypes are 1..=4");
    }
    // Sanity: the closest is well under the initial ring, so the query resolved in the first pass.
    assert!(out[0].distance_m < 1_000, "the nearest water POI is close");
}
