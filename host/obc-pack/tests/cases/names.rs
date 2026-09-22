//! What a packed region stores for a map-sourced name: pack a handful of synthetic places and
//! summits, then read the names back through the real reader.

use obc_elevation::NullElevation;
use obc_formats::obcm::{PoiMetadata, SourceId};
use obc_map_scene::BBox;
use obc_pack::poi::{classify, Poi};
use obc_pack::{serialize_lods, LodLayer, Node};
use obc_reader::{MapCache, MapTables, Reader, SliceSource};

/// A one-degree box (min_lon, min_lat, max_lon, max_lat) in microdegrees; every record sits well
/// inside it, so the single-leaf POI trees are reached by any view.
const GLOBAL: (i64, i64, i64, i64) = (23_000_000, 37_000_000, 24_000_000, 38_000_000);
const VIEW: BBox = BBox { min_lon: 23_000_000, min_lat: 37_000_000, max_lon: 24_000_000, max_lat: 38_000_000 };

/// Classify one hand-written tag set and place it on the grid, the way ingest does.
fn poi(id: u64, offset: i32, tags: &[(&str, &str)]) -> Poi {
    let classified = classify(tags.iter().copied()).expect("the tag set classifies");
    Poi {
        metadata: PoiMetadata { source: SourceId::osm(1, id), approach: None },
        access_nodes: Vec::new(),
        wikidata: None,
        wikipedia: None,
        subtype: classified.subtype,
        lon_udeg: 23_500_000 + offset,
        lat_udeg: 37_500_000 + offset,
        name: classified.name,
        from_node: true,
        hours: None,
        elevation_m: classified.elevation_m,
        population: classified.population,
    }
}

fn packed(pois: &[Poi]) -> Vec<u8> {
    let lod = LodLayer { max_mpp: None, chunk_size: 512, root: Node::Leaf { bbox: GLOBAL, features: Vec::new() } };
    let (bytes, dropped) = serialize_lods(
        &[lod],
        &[],
        0,
        GLOBAL,
        pois,
        &Default::default(),
        &obc_pack::config::default_profiles(),
        &mut NullElevation,
    );
    assert_eq!(dropped, 0, "the records fit their chunk");
    bytes
}

#[test]
fn a_packed_region_stores_names_in_the_device_repertoire() {
    let bytes = packed(&[
        poi(1, 0, &[("place", "town"), ("name", "Ρέθυμνο")]),
        poi(2, 1_000, &[("place", "village"), ("name", "Ploiești")]),
        poi(3, 2_000, &[("place", "village"), ("name", "Grüßau")]),
        poi(4, 3_000, &[("place", "village"), ("name", "東京"), ("name:en", "Tokyo")]),
        poi(5, 4_000, &[("natural", "peak"), ("name", "Эльбрус"), ("ele", "5642")]),
    ]);
    let source = SliceSource(&bytes);
    let tables = MapTables::parse(&source).expect("the packed region parses");
    let cache = MapCache::new();
    let reader = Reader::new(&source, &tables, &cache);

    let mut settlements = Vec::new();
    reader.visit_settlements_in(&VIEW, |s| settlements.push(s.name.to_string())).expect("settlement query");
    settlements.sort();
    assert_eq!(settlements, ["Grüßau", "Ploiesti", "Rethymno", "Tokyo"]);

    let mut summits = Vec::new();
    reader
        .visit_summits_within((23_504_000, 37_504_000), 1_000, |s| summits.push(s.name.to_string()))
        .expect("summit query");
    assert_eq!(summits, ["Elbrus"]);
}
