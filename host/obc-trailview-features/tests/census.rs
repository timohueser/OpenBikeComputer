//! The census over three cells of the TrailView fixture: a worker that panics and one that hangs each
//! end on their cell and the run goes on, and the cell between them builds as the viewer builds it. A
//! resume ends a partial last line, builds the timed-out cell once more and the panicked one never.
//! Workers that end before their first cell stop the run.

#[path = "../../obc-ground-plan/tests/fixture/mod.rs"]
mod fixture;

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output};

use obc_ground_plan::cell::build_cell;
use obc_ground_plan::faces::Rules;
use obc_ground_plan::frame::CellKey;
use obc_ground_plan::transport::Flat;
use serde_json::Value;

/// Three cells in a row: the first panics, the third hangs; the middle one has ways.
const ROW: [CellKey; 3] =
    [CellKey { x: 34304, y: 23104 }, CellKey { x: 34305, y: 23104 }, CellKey { x: 34306, y: 23104 }];

fn census(out: &Path, fail: &[String]) -> Output {
    let cells = ROW.map(|k| format!("{}/{}", k.x, k.y)).join(",");
    let mut command = Command::new(env!("CARGO_BIN_EXE_census"));
    command.args(["--fixture", fixture::default_dir().to_str().unwrap(), "--out", out.to_str().unwrap()]);
    command.args([
        "--memory",
        "none",
        "--workers",
        "1",
        "--timeout-s",
        "3",
        "--min-available-gb",
        "0",
        "--cells",
        &cells,
    ]);
    command.args(fail.iter().flat_map(|f| ["--fail", f.as_str()])).output().unwrap()
}

/// The lines of `census.jsonl` that parse, by cell, and the count of all lines.
fn lines(out: &Path) -> (BTreeMap<String, Value>, usize) {
    let text = std::fs::read_to_string(out.join("census.jsonl")).unwrap();
    let parsed = text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter(|v| v["cell"].is_array());
    (parsed.map(|v| (format!("{}/{}", v["cell"][0], v["cell"][1]), v)).collect(), text.lines().count())
}

#[test]
fn a_failed_cell_ends_its_worker_not_the_census_and_a_cell_builds_as_in_the_viewer() {
    let out = tempfile::tempdir().unwrap();
    let fail = [format!("{}/{}=panic", ROW[0].x, ROW[0].y), format!("{}/{}=hang", ROW[2].x, ROW[2].y)];
    let run = census(out.path(), &fail);
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
    let (first, count) = lines(out.path());
    let state = |l: &BTreeMap<String, Value>, k: CellKey| l[&format!("{}/{}", k.x, k.y)]["state"].clone();
    assert_eq!(ROW.map(|k| state(&first, k)), ["panic", "ok", "timeout"], "{first:#?}");
    let panic = &first["34304/23104"]["panic"];
    assert!(
        panic["message"].as_str().unwrap().contains("made-up") && panic["at"].as_str().unwrap().contains("worker.rs")
    );
    // The viewer's build of the fixture's request: the census reads and builds the cell alike.
    let request = fixture::Fixture::open(&fixture::default_dir()).request(ROW[1]).encode();
    let built = build_cell(&request, Rules::embedded(), &mut || 0.0).unwrap();
    assert_eq!(first["34305/23104"]["digest"].as_str(), Some(Flat::of(&built).digest(&built).unwrap().as_str()));
    // A driver that died in a line left part of it: the resume ends it, so the next line stays whole.
    let mut file = std::fs::OpenOptions::new().append(true).open(out.path().join("census.jsonl")).unwrap();
    file.write_all(br#"{"cell":[34305,2"#).unwrap();
    let run = census(out.path(), &fail);
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
    let (second, total) = lines(out.path());
    assert_eq!(total, count + 2, "the partial line and one retry of the timed-out cell");
    assert_eq!(
        (second["34306/23104"]["state"].as_str(), second["34306/23104"]["attempt"].as_u64()),
        (Some("timeout"), Some(2))
    );
    assert_eq!(second["34304/23104"], first["34304/23104"], "a panic is the cell's own: no retry");
    // Workers that cannot start stop the run after a few tries.
    let gone = tempfile::tempdir().unwrap();
    let run = census(gone.path(), &["start".to_string()]);
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(!run.status.success() && stderr.contains("before their first cell"), "{stderr}");
}
