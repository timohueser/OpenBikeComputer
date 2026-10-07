use super::*;
use crate::maps::tests::{fetched, root, temp};
use obc_data::engine::plan::plan;
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
    let env = Env { region: "box".into(), ..Env::default() };
    let mut wanted = Vec::new();
    assert!(resolve(&env, &regions, &store, &mut wanted).unwrap().is_none());
    assert_eq!(
        wanted,
        [Wanted { source: POLY.into(), version: None, params: vec![("area".into(), "europe/parent".into())] }]
    );
    assert!(
        !env.read.borrow().keys().any(|(source, _)| source == EXTRACTS),
        "coverage precedes bulk request selection"
    );
    source(&store, "europe/parent", "2026-03-01", "2", &poly(7.0, 9.0));
    let selected = resolve(&env, &regions, &store, &mut Vec::new()).unwrap().unwrap();
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
        resolve(
            &Env { region: "box".into(), ..Env::default() },
            &Regions::new(vec![region]).unwrap(),
            &store,
            &mut Vec::new(),
        )
    };
    let sources = selected("7.5,47.25,8.5,47.75").unwrap().unwrap().sources;
    assert_eq!(sources.iter().map(|source| source.id.as_str()).collect::<Vec<_>>(), ["europe/east", "europe/west"]);
    assert!(
        matches!(selected("6.5,47.25,8.5,47.75"), Err(Unplanned::Invalid(reason)) if reason.contains("complete box"))
    );
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
        .map(|(id, area)| (id.clone(), Coverage::parse_poly(&crate::maps::catalog::poly(area)).unwrap()))
        .collect();
    let selected = |west: f64, east: f64| {
        let requested = Coverage::parse_poly(&poly(west, east)).unwrap();
        select_box(&index, &shapes, &requested).unwrap().into_iter().collect::<Vec<_>>()
    };
    assert_eq!(selected(7.25, 7.75), ["europe/west"]);
    assert_eq!(selected(7.5, 8.5), ["europe/east", "europe/west"]);
}

#[test]
fn separate_input_layers_retain_distinct_versions_and_exact_files_in_release_provenance() {
    let temporary = temp("multi-source-provenance");
    let store = Store::at(temporary.0.join("store"));
    source(&store, "europe/west", "2026-01-01", "10", &poly(7.0, 8.0));
    source(&store, "europe/east", "2026-02-01", "20", &poly(8.0, 9.0));
    let region =
        parse_region("ride", "name='Ride'\nkind='geofabrik'\nareas=['europe/west','europe/east','europe/west']\n")
            .unwrap();
    let env = Env { region: "ride".into(), ..Env::default() };
    let selected = resolve(&env, &Regions::new(vec![region]).unwrap(), &store, &mut Vec::new()).unwrap().unwrap();
    let steps = inputs("maps", &selected);
    assert_eq!(steps.len(), 2);
    let root = root();
    let plan = plan(&store, &root, &steps).unwrap();
    assert!(plan.fetches().is_empty());
    let http = Http::new();
    let mut journal = Journal::create(&store, "build inputs").unwrap();
    journal
        .build(
            &Context { root: &root, store: &store, sources: &[], http: &http, copies: None, limits: Limits::machine() },
            &steps,
            &plan,
        )
        .unwrap();
    journal.finish(None).unwrap();
    let release = obc_data::engine::release::release(&store, &root, "maps", "ride", &[], &steps).unwrap().unwrap();
    assert!(release.objects().is_empty(), "raw input files remain private");
    let live = obc_data::live::Live {
        products: vec![obc_data::live::LiveProduct {
            product: "maps".into(),
            prefix: "cell-catalog".into(),
            release: Some((release.id(), release)),
            applied: None,
            document: None,
            observed: None,
        }],
        ..Default::default()
    };
    let reads = obc_data::input_copy::reads(&live).unwrap();
    let extracts = reads.iter().filter(|read| read.key.source == EXTRACTS).collect::<Vec<_>>();
    assert_eq!(extracts.len(), 2);
    assert!(extracts.iter().any(|read| read.key.version == "2026-01-01" && read.files == ["europe/west.osm.pbf"]));
    assert!(extracts.iter().any(|read| read.key.version == "2026-02-01" && read.files == ["europe/east.osm.pbf"]));
    assert_eq!(env.read.borrow().iter().filter(|((source, _), _)| source == EXTRACTS).count(), 2);
}
