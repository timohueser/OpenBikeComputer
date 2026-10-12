//! A detached update: the first process makes the review and starts the run in a process of its own,
//! which takes the lock, makes the review again and writes its status. The binary links no
//! product, so its release has no pointers, and the run fails at its build.

use std::path::PathBuf;
use std::process::{Command, Output};

struct Repo(PathBuf);

impl Repo {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("obc-data-detach-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("repo/data")).unwrap();
        std::fs::write(dir.join("repo/data/sources.toml"), "source = []\n").unwrap();
        let targets = format!("[local]\nkind = \"dir\"\ndir = \"{}\"\n", dir.join("local").display());
        std::fs::write(dir.join("repo/data/targets.toml"), targets).unwrap();
        Self(dir)
    }

    fn data(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_obc-data-plumbing"))
            .args(args)
            .current_dir(self.0.join("repo"))
            .env("OBC_DATA_STORE", self.0.join("store"))
            .output()
            .unwrap()
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The JSON documents of `output`, one per line, or one document.
fn json(output: &Output) -> Vec<serde_json::Value> {
    let text = String::from_utf8_lossy(&output.stdout);
    let lines: Result<Vec<_>, _> = text.lines().map(serde_json::from_str).collect();
    lines.or_else(|_| serde_json::from_str(&text).map(|one| vec![one])).unwrap_or_else(|e| panic!("{e}: {text}"))
}

#[test]
fn an_update_runs_detached_with_its_review_and_a_follower_gets_its_end() {
    let repo = Repo::new();
    let started = repo.data(&["--json", "update", "local", "--box", "7,47,8,48"]);
    assert!(started.status.success(), "{}", String::from_utf8_lossy(&started.stderr));
    let started = json(&started).remove(0);
    let run = started["run"].as_str().unwrap();

    let followed = repo.data(&["--json", "runs", run, "--follow"]);
    assert_eq!(followed.status.code(), Some(1), "the run failed");
    let lines = json(&followed);
    let (status, error) = (&lines[lines.len() - 2], &lines[lines.len() - 1]);
    assert_eq!((&status["state"], &error["error"]["run"]), (&"failed".into(), &run.into()));
    let review = started["review"].as_str().unwrap();
    let args = ["update", "local", "--box", "7,47,8,48", "--expect", review];
    assert_eq!(status["args"], serde_json::json!(args), "the run does the review that the first process made");
    assert_eq!(status["phases"][1]["phase"], "build");
    assert!(status["error"]["message"].as_str().unwrap().contains("pointers"), "{status}");

    let stop = repo.data(&["runs", run, "--stop"]);
    assert!(String::from_utf8_lossy(&stop.stderr).contains("does not run"));
}
