#![cfg(feature = "external-fixtures")]
use obc_reader::{MapCache, MapTables, Poi, PoiCategory, Reader, SliceSource, MAX_POI_RESULTS, MAX_SUMMIT_RADIUS_M};

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

// === Name repertoire =========================================================

/// Every settlement and summit name in a real baked map is drawable on the device.
///
/// Settlements and summits are the two record kinds that keep their UTF-8 spelling, so they are
/// the only ones a character outside the font could reach. `glyph_supported` reads the real font
/// strip, so this asserts the bake and the device agree about the repertoire.
#[test]
fn baked_maps_hold_no_name_the_font_cannot_draw() {
    for (package, file, settlements) in [("sim-freiburg", "freiburg.obcm", 44), ("sim-grimsel", "grimsel.obcm", 19)] {
        let bytes = obc_fixtures::read(package, file);
        let src = SliceSource(&bytes);
        let tables = MapTables::parse(&src).unwrap();
        let cache = MapCache::new();
        let r = Reader::new(&src, &tables, &cache);
        let view = tables.bbox;

        let check = |name: &str| {
            for c in name.chars() {
                assert!(obc_render::glyph_supported(c), "{package}: {name:?} holds {c:?}, which draws as `?`");
            }
        };

        let mut seen = 0;
        r.visit_settlements_in(&view, |s| {
            check(&s.name);
            seen += 1;
        })
        .unwrap();
        assert_eq!(seen, settlements, "{package} carries the settlements its provenance records");

        let centre = ((view.min_lon + view.max_lon) / 2, (view.min_lat + view.max_lat) / 2);
        r.visit_summits_within(centre, MAX_SUMMIT_RADIUS_M, |s| check(&s.name)).unwrap();
    }
}
