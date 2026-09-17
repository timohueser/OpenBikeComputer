use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn journey_trace_preserves_frames_and_retains_failure_evidence() {
    let scratch = Scratch(std::env::temp_dir().join(format!(
        "obc-diagnostics-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos(),
    )));
    fs::create_dir(&scratch.0).unwrap();
    fs::write(scratch.0.join("map.obcm"), super::map_bytes()).unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_obc-sim"))
            .current_dir(&scratch.0)
            .args(["map.obcm", "--boot", "--script", "Q f"])
            .args(args)
            .output()
            .unwrap()
    };
    for args in [vec!["--png", "plain.png"], vec!["--png", "traced.png", "--diagnostics", "trace.jsonl"]] {
        let result = run(&args);
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    }
    assert_eq!(fs::read(scratch.0.join("plain.png")).unwrap(), fs::read(scratch.0.join("traced.png")).unwrap());
    let trace = fs::read_to_string(scratch.0.join("trace.jsonl")).unwrap();
    let records: Vec<Value> = trace.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    for (index, record) in records.iter().enumerate() {
        assert_eq!(record["seq"], index);
    }
    assert_eq!(records.first().unwrap()["event"], "session");
    assert_eq!(records.last().unwrap()["event"], "finished");
    let before = records.iter().find(|row| row["event"] == "input" && row["data"]["token"] == "Q").unwrap();
    let after = records.iter().find(|row| row["event"] == "input_result" && row["data"]["token"] == "Q").unwrap();
    assert_ne!(before["data"]["screen"], after["data"]["screen"]);
    assert!(records
        .iter()
        .any(|row| row["event"] == "pass_output" && row["data"]["plan"].as_str().unwrap().contains("ReadCatalog")));
    assert!(records
        .iter()
        .any(|row| row["event"] == "executed" && row["data"]["outcomes"].as_str().unwrap().contains("CatalogRead")));
    assert!(records.iter().any(|row| row["event"] == "feeder"));
    assert!(records.iter().any(|row| row["event"] == "map"));

    // A new invocation must not replace a previous trace.
    assert!(!run(&["--png", "unused.png", "--diagnostics", "trace.jsonl"]).status.success());
    assert_eq!(fs::read_to_string(scratch.0.join("trace.jsonl")).unwrap(), trace);
    assert!(!scratch.0.join("unused.png").exists());

    let failed = run(&["--png", "failed.png", "--diagnostics", "failed.jsonl", "--expect-screen", "MissingScreen"]);
    assert!(!failed.status.success());
    let failed = fs::read_to_string(scratch.0.join("failed.jsonl")).unwrap();
    let last: Value = serde_json::from_str(failed.lines().last().unwrap()).unwrap();
    assert_eq!(last["event"], "failed");
    assert_eq!(last["data"]["expected_screen"], "MissingScreen");
    assert!(!scratch.0.join("failed.png").exists());
}
