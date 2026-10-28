use super::*;
use crate::device::tests::{fetched, root, temp};
use obc_data::engine::plan::{outputs, plan, Kept};
use obc_data::engine::runs::{Context, Limits, Run as Journal};
use obc_data::fetch::http::Http;
use obc_data::regions::parse_region;

fn fixture_index(store: &Store) {
    let feature = |id: &str, parent: Option<&str>, west: f64, east: f64, country: &str| {
        json!({
            "type":"Feature", "properties":{"id":id, "name":id, "parent":parent,
                "iso3166-1:alpha2": [country],
                "urls":{"pbf":format!("https://download.geofabrik.de/europe/{id}-latest.osm.pbf")}},
            "geometry":{"type":"Polygon", "coordinates":[[[west,47.0],[east,47.0],[east,48.0],[west,48.0],[west,47.0]]]}
        })
    };
    let body = json!({"type":"FeatureCollection", "features":[
        feature("parent", None, 7.0, 9.0, "DE"),
        feature("west", Some("parent"), 7.0, 8.0, "DE"),
        feature("east", Some("parent"), 8.0, 9.0, "CH")
    ]});
    fetched(store, INDEX, "1", &[], &[("index.json".into(), body.to_string())]);
}

fn poly(west: f64, east: f64) -> String {
    format!("source\n1\n {west} 47\n {east} 47\n {east} 48\n {west} 48\n {west} 47\nEND\nEND\n")
}

fn source(store: &Store, id: &str, extract: &str, poly_version: &str, shape: &str) {
    let params = [("area".into(), id.into())];
    fetched(store, POLY, poly_version, &params, &[(format!("{id}.poly"), shape.into())]);
    fetched(store, EXTRACTS, extract, &params, &[(format!("{id}.osm.pbf"), format!("{id} {extract}"))]);
}

#[test]
fn box_descent_uses_actual_geometry_and_full_poly_gaps_fall_back_before_bulk_fetch() {
    let temporary = temp("box-source-coverage");
    let store = Store::at(temporary.0.join("store"));
    fixture_index(&store);
    source(&store, "europe/west", "2026-01-01", "1", &poly(7.0, 7.99));
    source(&store, "europe/east", "2026-02-01", "1", &poly(8.0, 9.0));
    let region = parse_region("box", "name='Box'\nkind='box'\nbox=[7.5,47.25,8.5,47.75]\ncountries=['DE']\n").unwrap();
    let regions = Regions::new(vec![region]).unwrap();
    let inputs = inputs("box");
    let mut wanted = Vec::new();
    assert!(resolve(&inputs, &regions, &store, &mut wanted).unwrap().is_none());
    assert_eq!(
        wanted,
        [Wanted { source: POLY.into(), version: None, params: vec![("area".into(), "europe/parent".into())] }]
    );
    assert!(
        !inputs.requests.borrow().iter().any(|(source, _)| source == EXTRACTS),
        "coverage precedes bulk request selection"
    );
    source(&store, "europe/parent", "2026-03-01", "2", &poly(7.0, 9.0));
    let selected = resolve(&inputs, &regions, &store, &mut Vec::new()).unwrap().unwrap();
    assert_eq!(selected.sources.iter().map(|source| source.id.as_str()).collect::<Vec<_>>(), ["europe/parent"]);
    assert!(selected.coverage.covers_coverage(&selected.outlines[0]).unwrap());
    assert_eq!(selected.index.as_deref(), Some("1"));
    assert_eq!(selected.outlines[0].bbox(), (7_500_000, 47_250_000, 8_500_000, 47_750_000));
}

#[test]
fn a_box_descends_into_both_countries_and_rejects_uncovered_ground() {
    let temporary = temp("box-source-descent");
    let store = Store::at(temporary.0.join("store"));
    fixture_index(&store);
    source(&store, "europe/west", "2026-01-01", "1", &poly(7.0, 8.0));
    source(&store, "europe/east", "2026-02-01", "1", &poly(8.0, 9.0));
    let selected = |bounds: &str| {
        let region =
            parse_region("box", &format!("name='Box'\nkind='box'\nbox=[{bounds}]\ncountries=['DE']\n")).unwrap();
        resolve(&inputs("box"), &Regions::new(vec![region]).unwrap(), &store, &mut Vec::new())
    };
    let sources = selected("7.5,47.25,8.5,47.75").unwrap().unwrap().sources;
    assert_eq!(sources.iter().map(|source| source.id.as_str()).collect::<Vec<_>>(), ["europe/east", "europe/west"]);
    assert!(
        matches!(selected("6.5,47.25,8.5,47.75"), Err(Unplanned::Invalid(reason)) if reason.contains("complete box"))
    );
}

/// The outlines of a union, which give the planner its bounds and its cut, are its box and the
/// areas that it names: not the extract that its box selects.
#[test]
fn a_union_of_an_area_and_a_box_is_outlined_by_its_parts() {
    let temporary = temp("union-source-outlines");
    let store = Store::at(temporary.0.join("store"));
    fixture_index(&store);
    source(&store, "europe/west", "2026-01-01", "1", &poly(7.0, 8.0));
    source(&store, "europe/east", "2026-02-01", "1", &poly(8.0, 9.0));
    let regions = Regions::new(vec![
        parse_region("west", "name='West'\nkind='geofabrik'\nareas=['europe/west']\ncountries=['DE']\n").unwrap(),
        parse_region("box", "name='Box'\nkind='box'\nbox=[8.2,47.2,8.6,47.6]\ncountries=['CH']\n").unwrap(),
        parse_region("both", "name='Both'\nkind='union'\nunion=['west','box']\ncountries=['DE']\n").unwrap(),
    ])
    .unwrap();
    let selected = resolve(&inputs("both"), &regions, &store, &mut Vec::new()).unwrap().unwrap();
    assert_eq!(
        selected.sources.iter().map(|source| source.id.as_str()).collect::<Vec<_>>(),
        ["europe/east", "europe/west"]
    );
    let outlines = Coverage::union(&selected.outlines.iter().collect::<Vec<_>>()).unwrap();
    assert_eq!(outlines.bbox(), (7_000_000, 47_000_000, 8_600_000, 48_000_000));
}

#[test]
fn a_box_skips_an_aggregate_that_overlaps_its_sibling_countries() {
    let feature = |id: &str, parent: Option<&str>, west: f64, east: f64| {
        json!({
            "type":"Feature", "properties":{"id":id, "name":id, "parent":parent,
                "urls":{"pbf":format!("https://download.geofabrik.de/europe/{id}-latest.osm.pbf")}},
            "geometry":{"type":"Polygon", "coordinates":[[[west,47.0],[east,47.0],[east,48.0],[west,48.0],[west,47.0]]]}
        })
    };
    let body = json!({"type":"FeatureCollection", "features":[
        feature("parent", None, 7.0, 9.0),
        feature("west", Some("parent"), 7.0, 8.0),
        feature("east", Some("parent"), 8.0, 9.0),
        feature("dach", Some("parent"), 7.0, 9.0)
    ]});
    let index = geofabrik::parse(body.to_string().as_bytes()).unwrap();
    let shapes = index
        .iter()
        .map(|(id, area)| (id.clone(), Coverage::parse_poly(&crate::device::catalog::poly(area)).unwrap()))
        .collect();
    let selected = |west: f64, east: f64| {
        let requested = Coverage::parse_poly(&poly(west, east)).unwrap();
        select_box(&shapes, &requested).unwrap().into_iter().collect::<Vec<_>>()
    };
    assert_eq!(selected(7.25, 7.75), ["europe/west"]);
    assert_eq!(selected(7.5, 8.5), ["europe/east", "europe/west"]);
}

/// The children of `west` stop at 8.9 °E, and `west` ends at 9 °E: a child of `east` covers that
/// sliver of a box that also reaches into `east`.
#[test]
fn a_box_takes_a_neighbour_for_a_sliver_before_the_parent() {
    let feature = |id: &str, parent: Option<&str>, [west, south, east, north]: [f64; 4]| {
        json!({
            "type":"Feature", "properties":{"id":id, "name":id, "parent":parent,
                "urls":{"pbf":format!("https://download.geofabrik.de/europe/{id}-latest.osm.pbf")}},
            "geometry":{"type":"Polygon", "coordinates":[[[west,south],[east,south],[east,north],[west,north],[west,south]]]}
        })
    };
    let body = json!({"type":"FeatureCollection", "features":[
        feature("west", None, [7.0, 47.0, 9.0, 48.0]),
        feature("west/a", Some("west"), [7.0, 47.0, 8.0, 48.0]),
        feature("west/b", Some("west"), [8.0, 47.0, 8.9, 48.0]),
        feature("east", None, [8.8, 47.0, 10.0, 47.5]),
        feature("east/c", Some("east"), [8.8, 47.0, 10.0, 47.4])
    ]});
    let index = geofabrik::parse(body.to_string().as_bytes()).unwrap();
    let shapes = index
        .iter()
        .map(|(id, area)| (id.clone(), Coverage::parse_poly(&crate::device::catalog::poly(area)).unwrap()))
        .collect();
    let selected = |bbox: [f64; 4]| {
        let requested = Coverage::parse_poly(&box_poly(&obc_data::regions::Bbox::new(bbox).unwrap())).unwrap();
        select_box(&shapes, &requested).unwrap().into_iter().collect::<Vec<_>>()
    };
    assert_eq!(selected([8.5, 47.1, 9.5, 47.3]), ["europe/east/c", "europe/west/b"]);
    // North of `east`, only the parent covers the sliver.
    assert_eq!(selected([8.5, 47.6, 8.95, 47.9]), ["europe/west"]);
}

#[test]
fn separate_input_layers_retain_distinct_versions_in_release_provenance() {
    let temporary = temp("multi-source-provenance");
    let store = Store::at(temporary.0.join("store"));
    source(&store, "europe/west", "2026-01-01", "10", &poly(7.0, 8.0));
    source(&store, "europe/east", "2026-02-01", "20", &poly(8.0, 9.0));
    let region =
        parse_region("ride", "name='Ride'\nkind='geofabrik'\nareas=['europe/west','europe/east','europe/west']\n")
            .unwrap();
    let inputs = inputs("ride");
    let selected = resolve(&inputs, &Regions::new(vec![region]).unwrap(), &store, &mut Vec::new()).unwrap().unwrap();
    let steps = super::inputs("device", &selected);
    assert_eq!(steps.len(), 2);
    let root = root();
    let plan = plan(&store, &steps).unwrap();
    assert!(plan.fetches().is_empty());
    let (http, kept) = (Http::new(), Kept::new());
    let context = Context {
        root: &root,
        store: &store,
        sources: &[],
        http: &http,
        kept: &kept,
        served: &Default::default(),
        limits: Limits::machine(),
    };
    let mut journal = Journal::create(&store, ["build", "inputs"]).unwrap();
    journal.build(&context, &steps, &plan).unwrap();
    journal.finish(None).unwrap();
    let outputs = outputs(&store, &steps, &kept).unwrap().unwrap();
    assert!(outputs.iter().all(|output| output.client_files().next().is_none()), "raw input files remain private");
    let extracts: Vec<_> = outputs.iter().map(|output| &output.snapshots[EXTRACTS]).collect();
    assert!(extracts.iter().any(|read| read.version == "2026-01-01" && read.params[0].1 == "europe/west"));
    assert!(extracts.iter().any(|read| read.version == "2026-02-01" && read.params[0].1 == "europe/east"));
    assert_eq!(inputs.requests.borrow().iter().filter(|(source, _)| source == EXTRACTS).count(), 2);
}

/// The inputs of `region` without a serving release.
fn inputs(region: &str) -> Inputs {
    Inputs {
        settings: obc_data::settings::Settings { region: region.into(), ..Default::default() },
        ..Inputs::default()
    }
}
