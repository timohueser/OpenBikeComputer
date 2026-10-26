//! `obc data source credential`: the value comes on standard input, goes to the credentials file
//! under HOME, and shows in no output; `obc data setup` then has the credential.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

const VALUE: &str = "fake-token-not-a-real-credential";

fn data(dir: &Path, args: &[&str], input: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_obc-data-plumbing"))
        .args(args)
        .current_dir(dir.join("repo"))
        .env("HOME", dir.join("home"))
        .env("OBC_DATA_STORE", dir.join("store"))
        .env_remove("OBC_TEST_TOKEN")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    child.wait_with_output().unwrap()
}

struct Scratch(std::path::PathBuf);

impl Scratch {
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_credential_from_standard_input_is_set_and_never_shown() {
    let dir = Scratch(std::env::temp_dir().join(format!("obc-data-credential-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(dir.path());
    std::fs::create_dir_all(dir.path().join("repo/data")).unwrap();
    let source = "[[source]]\nid = \"keyed\"\nkind = \"data\"\nused_by = [\"planner\"]\nuse = \"A test layer\"\n\
                  fetch = { kind = \"http\", url = \"https://127.0.0.1:9/keyed.zip\" }\nversion = \"digest\"\n\
                  redistribute = false\ncredential = { env = [\"OBC_TEST_TOKEN\"] }\n";
    std::fs::write(dir.path().join("repo/data/sources.toml"), source).unwrap();
    let state = |dir: &Path| {
        let setup = data(dir, &["--json", "setup"], "");
        let setup: serde_json::Value = serde_json::from_slice(&setup.stdout).unwrap();
        let keyed = setup["credentials"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["set"] == serde_json::json!(["source", "credential", "keyed"]))
            .unwrap();
        (keyed["state"].as_str().unwrap().to_string(), keyed["fields"].clone())
    };
    assert_eq!(state(dir.path()), ("optional".into(), serde_json::json!(["OBC_TEST_TOKEN"])));

    for args in [&["--json", "source", "credential", "keyed"][..], &["source", "credential", "keyed"]] {
        let set = data(dir.path(), args, &format!("{VALUE}\n"));
        let (stdout, stderr) = (String::from_utf8_lossy(&set.stdout), String::from_utf8_lossy(&set.stderr));
        assert!(set.status.success(), "{stdout}{stderr}");
        assert!(!stdout.contains(VALUE) && !stderr.contains(VALUE), "{stdout}{stderr}");
        assert!(stdout.contains("OBC_TEST_TOKEN"), "{stdout}");
    }
    let file = dir.path().join("home/.config/openbikecomputer/credentials.env");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), format!("OBC_TEST_TOKEN={VALUE}\n"));
    assert_eq!(state(dir.path()).0, "ready");
}
