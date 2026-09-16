//! Captured OSM topology through production ingest, OBCM serialization and paged reader.
#![cfg(feature = "external-fixtures")]

use obc_formats::obcm::{PoiCategory, SourceId};
use obc_pack::config::Config;
use obc_pack::ingest::{ingest_osm, Bbox};
use obc_pack::progress::Progress;
use obc_pack::{serialize_lods, LodLayer, Node};
use obc_reader::reader::places::{PlaceQuery, PlaceWindow, QueryProgress};
use obc_reader::{MapCache, MapTables, PoiCategorySet, Reader, SliceSource};

#[test]
#[ignore = "manual captured-source suite: fixtures/verify-assistant-places.py"]
fn explicit_gletsch_access_survives_packing_and_nearby_roads_do_not_create_access() {
    let source = obc_fixtures::file_in("assistant-inputs", "assistant-osm", "switzerland.osm.pbf");
    let config = Config::parse(include_str!("../../../builder/presets/schema.json")).unwrap();
    let ing = ingest_osm(
        &[source.display().to_string()],
        &config,
        Some(Bbox::parse("8.17,46.55,8.38,46.74").unwrap()),
        &Progress::silent(),
    )
    .unwrap();
    let station = SourceId::osm(1, 420_041_304);
    let negative_station = SourceId::osm(1, 5_007_055_738);
    let negative_water = SourceId::osm(1, 6_781_569_985);
    let link = ing.landmark_links.iter().find(|link| link.metadata.source == station).unwrap();
    assert_eq!(link.wikidata.as_deref(), Some("Q5569509"));
    let access = link.metadata.approach.expect("station source node is on service way w231897254");
    assert_eq!(access.source, station);
    assert_eq!((access.lon, access.lat), (8_361_496, 46_561_323));
    assert_ne!(access.profile_mask, 0);
    for id in [negative_station, negative_water] {
        assert!(ing.pois.iter().find(|poi| poi.metadata.source == id).unwrap().metadata.approach.is_none());
    }
    let bbox = obc_pack::pipeline::compute_bbox(&ing);
    let lods = [LodLayer { max_mpp: None, chunk_size: 512, root: Node::Leaf { bbox, features: vec![] } }];
    let (bytes, dropped) = serialize_lods(
        &lods,
        &[],
        config.marker_color,
        bbox,
        &ing.pois,
        &ing.nav_graph,
        &config.routing.profiles,
        &mut obc_elevation::NullElevation,
    );
    assert_eq!(dropped, 0);
    let source = SliceSource(&bytes);
    let tables = MapTables::parse(&source).unwrap();
    let cache = MapCache::new_boxed();
    let reader = Reader::new(&source, &tables, &cache);
    let mut query = PlaceQuery::new(
        1,
        PoiCategorySet::ALL,
        PlaceWindow::Nearby { position: (8_184_271, 46_727_359), radius_m: 30_000 },
        None,
    );
    let mut page = heapless::Vec::<_, { obc_reader::reader::places::PLACE_PAGE_SIZE }>::new();
    let mut found = std::collections::BTreeMap::new();
    let mut pages = 0;
    loop {
        let state = loop {
            let state = query.step(&reader, None, 1, &mut page);
            if state != QueryProgress::Pending {
                break state;
            }
        };
        pages += 1;
        for hit in &page {
            found.insert(hit.poi.metadata.source, hit.poi.clone());
        }
        let QueryProgress::Ready { more, coverage_complete: false } = state else { panic!("{state:?}") };
        if !more {
            break;
        }
        query.next_page(query.key(page.last().unwrap()));
        page.clear();
    }
    assert!(pages > 1, "actual regional data exercises continuation past sixteen");
    assert_eq!(found[&station].metadata.approach, Some(access));
    assert_eq!(obc_formats::obcm::poi_category_of(found[&station].subtype), Some(PoiCategory::Train));
    assert!(found[&negative_station].metadata.approach.is_none());
    assert!(found[&negative_water].metadata.approach.is_none());
    eprintln!(
        "{} real service identities across {pages} pages; Gletsch explicit access; Meiringen station/water unavailable",
        found.len()
    );
}
