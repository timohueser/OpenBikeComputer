//! Contract tests for the settlement viewport query (`Reader::visit_settlements_in`).
//!
//! Each test builds a synthetic `.obcm` whose category-9 quadtree is a real POI tree (via
//! `obcm-testkit`'s `build_poi_map`), then asserts what the query gives back and how much it reads.

use crate::common::CountingSource;
use obc_formats::obcm::{
    SettlementClass, SETTLEMENT_CATEGORY_ID, SETTLEMENT_POPULATION_UNKNOWN, SETTLEMENT_SUBTYPE_CITY,
    SETTLEMENT_SUBTYPE_HAMLET, SETTLEMENT_SUBTYPE_TOWN, SETTLEMENT_SUBTYPE_VILLAGE, SUMMIT_CATEGORY_ID,
    SUMMIT_SUBTYPE_ID,
};
use obc_map_scene::BBox;
use obc_reader::{MapCache, MapTables, Reader, Settlement, SliceSource};
use obcm_testkit::{build_poi_map, PoiSpec};

const BBOX: (i32, i32, i32, i32) = (7_000_000, 43_000_000, 8_000_000, 44_000_000);
const CS: usize = 512;
const MID_LON: i32 = 7_500_000;
const MID_LAT: i32 = 43_500_000;

fn view(min_lon: i32, min_lat: i32, max_lon: i32, max_lat: i32) -> BBox {
    BBox { min_lon, min_lat, max_lon, max_lat }
}

const WHOLE_MAP: BBox = BBox { min_lon: BBOX.0, min_lat: BBOX.1, max_lon: BBOX.2, max_lat: BBOX.3 };

fn spec(lat: i32, lon: i32, subtype: u8, name: &str, payload: u16) -> PoiSpec {
    PoiSpec { lat, lon, subtype, name: name.into(), payload }
}

/// Run the query over `view` and collect what it visits.
fn settlements(map: &[u8], view: &BBox) -> Vec<Settlement> {
    let src = SliceSource(map);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let mut found = Vec::new();
    Reader::new(&src, &tables, &cache).visit_settlements_in(view, |s| found.push(s)).unwrap();
    found
}

/// The file offset of the record whose stored name is `name` — the anchor every corruption test
/// edits from.
fn record_at(map: &[u8], name: &str) -> usize {
    map.windows(name.len()).position(|w| w == name.as_bytes()).expect("stored name") - 10
}

/// Summits beside settlements is the directory a real v18 map has: nine entries, which is
/// `POI_MAX_CATEGORIES`. The summit category must not reach this query.
#[test]
fn settlements_inside_the_view_are_visited() {
    let map = build_poi_map(
        BBOX,
        CS,
        &[
            (
                SETTLEMENT_CATEGORY_ID,
                vec![
                    spec(43_700_000, 7_100_000, SETTLEMENT_SUBTYPE_CITY, "Freiburg", 2_300),
                    spec(43_710_000, 7_110_000, SETTLEMENT_SUBTYPE_TOWN, "Emmendingen", 280),
                    spec(43_720_000, 7_120_000, SETTLEMENT_SUBTYPE_VILLAGE, "Denzlingen", 130),
                    spec(43_730_000, 7_130_000, SETTLEMENT_SUBTYPE_HAMLET, "Hofsgrund", 3),
                ],
            ),
            (SUMMIT_CATEGORY_ID, vec![spec(43_740_000, 7_140_000, SUMMIT_SUBTYPE_ID, "Schauinsland", 1_284)]),
        ],
    );
    let mut found: Vec<_> = settlements(&map, &WHOLE_MAP)
        .into_iter()
        .map(|s| (s.class, s.name.to_string(), s.lat, s.lon, s.population))
        .collect();
    found.sort();
    assert_eq!(
        found,
        vec![
            (SettlementClass::City, "Freiburg".into(), 43_700_000, 7_100_000, Some(230_000)),
            (SettlementClass::Town, "Emmendingen".into(), 43_710_000, 7_110_000, Some(28_000)),
            (SettlementClass::Village, "Denzlingen".into(), 43_720_000, 7_120_000, Some(13_000)),
            (SettlementClass::Hamlet, "Hofsgrund".into(), 43_730_000, 7_130_000, Some(300)),
        ]
    );
}

#[test]
fn settlements_outside_the_view_are_not_visited() {
    // Both records share one leaf, so the leaf meets the view and the record check is the only wall.
    let map = build_poi_map(
        BBOX,
        CS,
        &[(
            SETTLEMENT_CATEGORY_ID,
            vec![
                spec(43_700_000, 7_100_000, SETTLEMENT_SUBTYPE_TOWN, "Inside", 40),
                spec(43_900_000, 7_400_000, SETTLEMENT_SUBTYPE_TOWN, "Outside", 40),
            ],
        )],
    );
    let names: Vec<_> = settlements(&map, &view(7_050_000, 43_650_000, 7_150_000, 43_750_000))
        .into_iter()
        .map(|s| s.name.to_string())
        .collect();
    assert_eq!(names, vec!["Inside".to_string()]);
}

#[test]
fn a_map_without_settlements_gives_no_results() {
    let map = build_poi_map(BBOX, CS, &[(1, vec![spec(43_700_000, 7_100_000, 1, "Brunnen", 0xFFFF)])]);
    assert!(settlements(&map, &WHOLE_MAP).is_empty());
}

/// The directory carries a category-9 entry with `node_count` 0, which the query must skip
/// exactly as it skips an absent category.
#[test]
fn an_empty_settlement_category_gives_no_results() {
    let map = build_poi_map(BBOX, CS, &[(SETTLEMENT_CATEGORY_ID, vec![])]);
    assert!(settlements(&map, &WHOLE_MAP).is_empty());
}

#[test]
fn a_settlement_name_keeps_its_diacritics() {
    let map = build_poi_map(BBOX, CS, &[(SETTLEMENT_CATEGORY_ID, vec![spec(MID_LAT, MID_LON, 23, "Grüßau", 4)])]);
    let found = settlements(&map, &WHOLE_MAP);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].name.as_str(), "Grüßau");
}

#[test]
fn a_corrupt_settlement_record_is_dropped_alone() {
    for corrupt in ["Badutf", "Nolenn", "Ctrlch"] {
        let map = build_poi_map(
            BBOX,
            CS,
            &[(
                SETTLEMENT_CATEGORY_ID,
                vec![
                    spec(43_700_000, 7_100_000, SETTLEMENT_SUBTYPE_TOWN, corrupt, 40),
                    spec(43_710_000, 7_110_000, SETTLEMENT_SUBTYPE_TOWN, "Good", 40),
                ],
            )],
        );
        let mut map = map;
        let at = record_at(&map, corrupt);
        match corrupt {
            "Badutf" => map[at + 10] = 0xF8, // never a valid UTF-8 lead byte
            "Nolenn" => map[at + 9] = 0,
            _ => map[at + 11] = 0x01,
        }
        let names: Vec<_> = settlements(&map, &WHOLE_MAP).into_iter().map(|s| s.name.to_string()).collect();
        assert_eq!(names, vec!["Good".to_string()], "{corrupt} must be dropped on its own");
    }
}

#[test]
fn an_unknown_population_reads_as_none() {
    let map = build_poi_map(
        BBOX,
        CS,
        &[(
            SETTLEMENT_CATEGORY_ID,
            vec![
                spec(43_700_000, 7_100_000, SETTLEMENT_SUBTYPE_CITY, "Known", 2_500),
                spec(43_710_000, 7_110_000, SETTLEMENT_SUBTYPE_CITY, "Unknown", SETTLEMENT_POPULATION_UNKNOWN),
            ],
        )],
    );
    let mut found: Vec<_> =
        settlements(&map, &WHOLE_MAP).into_iter().map(|s| (s.name.to_string(), s.population)).collect();
    found.sort();
    assert_eq!(found, vec![("Known".to_string(), Some(250_000)), ("Unknown".to_string(), None)]);
}

#[test]
fn a_settlement_query_reads_one_chunk_per_leaf() {
    // Ten records over two quadrants: the root splits, and two of its four children hold chunks.
    let mut specs = Vec::new();
    for i in 0..5 {
        specs.push(spec(MID_LAT + 100_000 + i, MID_LON - 100_000, SETTLEMENT_SUBTYPE_VILLAGE, &format!("NW{i}"), 4));
        specs.push(spec(MID_LAT - 100_000 - i, MID_LON + 100_000, SETTLEMENT_SUBTYPE_VILLAGE, &format!("SE{i}"), 4));
    }
    let map = build_poi_map(BBOX, CS, &[(SETTLEMENT_CATEGORY_ID, specs)]);

    // The first pass makes the small index resident, so the counted pass pays for chunks only.
    // Each view gets its own reader, because the leaf walk is cached per view.
    let counted = |view: &BBox| {
        let src = CountingSource::new(&map);
        let tables = MapTables::parse(&src).unwrap();
        let cache = MapCache::new();
        let reader = Reader::new(&src, &tables, &cache);
        let mut visited = 0;
        reader.visit_settlements_in(view, |_| visited += 1).unwrap();
        src.take();
        reader.visit_settlements_in(view, |_| ()).unwrap();
        (visited, src.take())
    };

    assert_eq!(counted(&WHOLE_MAP), (10, 2), "one chunk read per non-empty leaf");
    // Kept off the split lines: a node box that only touches the view still meets it.
    let quadrant = view(MID_LON - 200_000, MID_LAT + 1, MID_LON - 1, MID_LAT + 200_000);
    assert_eq!(counted(&quadrant), (5, 1), "only the leaves that meet the view are read");
}
