//! The census over three cells of the TrailView fixture: a worker that panics and one that hangs each
//! end on their cell and the run goes on, the cell between them builds as the viewer builds it, and a
//! second run resumes with nothing left to do.

#[path = "../../obc-ground-plan/tests/fixture/mod.rs"]
mod fixture;

use std::collections::BTreeMap;
use std::process::Command;

use obc_ground_plan::cell::build_cell;
use obc_ground_plan::faces::Rules;
use obc_ground_plan::frame::CellKey;
use obc_ground_plan::transport::Flat;
use serde_json::Value;

/// Three cells in a row: the first panics, the third hangs; the middle one has ways.
const ROW: [CellKey; 3] =
    [CellKey { x: 34304, y: 23104 }, CellKey { x: 34305, y: 23104 }, CellKey { x: 34306, y: 23104 }];

fn census(out: &std::path::Path) -> BTreeMap<String, Value> {
    let name = |k: CellKey| format!("{}/{}", k.x, k.y);
    let cells = ROW.map(name).join(",");
    let status = Command::new(env!("CARGO_BIN_EXE_census"))
        .args(["--fixture", fixture::default_dir().to_str().unwrap(), "--out", out.to_str().unwrap()])
        .args(["--memory", "none", "--workers", "1", "--timeout-s", "5", "--min-available-gb", "0", "--cells", &cells])
        .args(["--fail", &format!("{}=panic", name(ROW[0])), "--fail", &format!("{}=hang", name(ROW[2]))])
        .status()
        .unwrap();
    assert!(status.success(), "the census ended with {status}");
    let text = std::fs::read_to_string(out.join("census.jsonl")).unwrap();
    let lines = text.lines().map(|l| serde_json::from_str::<Value>(l).unwrap());
    lines.map(|v| (format!("{}/{}", v["cell"][0], v["cell"][1]), v)).collect()
}

#[test]
fn a_failed_cell_ends_its_worker_not_the_census_and_a_cell_builds_as_in_the_viewer() {
    let out = tempfile::tempdir().unwrap();
    let lines = census(out.path());
    let state = |k: CellKey| lines[&format!("{}/{}", k.x, k.y)]["state"].as_str().unwrap().to_string();
    assert_eq!(ROW.map(state), ["panic", "ok", "timeout"], "{lines:#?}");
    let panic = &lines["34304/23104"]["panic"];
    assert!(
        panic["message"].as_str().unwrap().contains("made-up") && panic["at"].as_str().unwrap().contains("worker.rs")
    );
    // The viewer's build of the fixture's request: the census reads and builds the cell alike.
    let request = fixture::Fixture::open(&fixture::default_dir()).request(ROW[1]).encode();
    let built = build_cell(&request, Rules::embedded(), &mut || 0.0).unwrap();
    assert_eq!(lines["34305/23104"]["digest"].as_str(), Some(Flat::of(&built).digest(&built).unwrap().as_str()));
    let summary: Value = serde_json::from_slice(&std::fs::read(out.path().join("summary.json")).unwrap()).unwrap();
    assert_eq!(summary["failed"].as_array().map(Vec::len), Some(2));
    // A second run finds every cell done.
    assert_eq!(census(out.path()).len(), lines.len());
    assert_eq!(std::fs::read_to_string(out.path().join("census.jsonl")).unwrap().lines().count(), lines.len());
}
