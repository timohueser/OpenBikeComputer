//! The phone's router over the web builder's cell fixture: two network cells across a seam and
//! three terrain cells. `route-vector.json` is the vector the OBCKit routing suite checks the
//! Swift side against; `OBC_REGENERATE=1` rewrites it.

use obc_companion_core::{assemble, CellMap, Job, RouteError};
use obc_route::BikeType;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../obc-web-assemble/tests/fixture")
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// The map the phone assembles: a catalog root, the network band only, and terrain.
fn map() -> CellMap {
    let dir = fixture();
    let cells = read_json(&dir.join("cells.json"));
    let terrain = read_json(&dir.join("terrain.json"));
    let catalog = json!({
        "schema": cells["schema"],
        "skins": [read_json(&dir.join("skin.json"))],
        "terrain": { "posting_log2": terrain["posting_log2"], "cell_log2": terrain["cell_log2"] },
    });
    let path = |p: &Value| dir.join(p.as_str().unwrap()).to_str().unwrap().to_owned();
    let job: Job = serde_json::from_value(json!({
        "cells": cells["cells"].as_array().unwrap().iter().filter(|c| c["band"] == "network").map(|c| json!({
            "id": c["id"], "band": c["band"], "partial": c["partial"], "path": path(&c["path"]),
        })).collect::<Vec<_>>(),
        "terrain": terrain["cells"].as_array().unwrap().iter().map(|c| json!({
            "id": c["id"], "sha256": c["sha256"], "path": path(&c["path"]),
        })).collect::<Vec<_>>(),
    }))
    .unwrap();
    assemble(&catalog.to_string(), job).unwrap()
}

const SEAM: i32 = 7_602_176;
const LAT: i32 = 47_300_000;

#[test]
fn a_route_across_the_seam_matches_the_vector() {
    let map = map();
    // Both endpoints are off the road, so both snap.
    let (from, to) = ((SEAM - 45_000, LAT + 300), (SEAM + 30_300, LAT + 59_000));
    let leg = map.route(from, to, BikeType::Road).unwrap();
    assert!(leg.points.first().unwrap().lon < SEAM && leg.points.last().unwrap().lon > SEAM);
    assert!(leg.points.iter().all(|p| p.ele.is_some()), "every point samples the fixture's terrain");

    let actual = json!({
        "from": [from.0, from.1],
        "to": [to.0, to.1],
        "profile": 0,
        "distance_m": leg.distance_m,
        "ascent_m": leg.ascent_m,
        "points": leg.points.iter().map(|p| json!([p.lon, p.lat, p.ele, p.surface, p.elevation_incomplete])).collect::<Vec<_>>(),
    });
    let vector = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/route-vector.json");
    if std::env::var_os("OBC_REGENERATE").is_some() {
        std::fs::write(&vector, serde_json::to_string_pretty(&actual).unwrap() + "\n").unwrap();
    }
    assert_eq!(actual, read_json(&vector), "the router changed; OBC_REGENERATE=1 rewrites the vector");
}

#[test]
fn failures_are_typed() {
    let map = map();
    let road = (SEAM - 45_000, LAT);
    // Two kilometres from any way.
    assert_eq!(map.route(road, (SEAM, LAT - 20_000), BikeType::Road), Err(RouteError::NoRoad));
    // The islet below the prune threshold is gone from the assembled graph.
    assert_eq!(map.route(road, (SEAM, LAT - 40_000), BikeType::Road), Err(RouteError::NoRoad));
}
