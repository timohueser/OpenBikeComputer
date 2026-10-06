//! Cargo chooses the worker artifact; its temporary copy leaves the build path writable.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use obc_data::worker;

pub fn run() -> Result<u8, String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let root = obc_data::find_root(&cwd).ok_or("obc data runs in a repository checkout")?;
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let code = worker::fingerprint(&root)?;
    let executable = build(&root)?;
    if worker::fingerprint(&root)? != code {
        return Err("Rust producer code changed during compilation; restart obc data".into());
    }
    let directory = tempfile::Builder::new().prefix("obc-data-worker-").tempdir().map_err(|e| e.to_string())?;
    let copy = copy_worker(&executable, directory.path())?;
    let sha256 = obc_data::store::hash_file(&copy)?.0;
    let status = Command::new(&copy)
        .args(std::env::args_os().skip(1))
        .env(worker::ROOT, &root)
        .env(worker::CODE, code)
        .env(worker::EXE, sha256)
        .status()
        .map_err(|e| format!("{}: {e}", copy.display()))?;
    // The child has exited, so Windows can remove the copied executable too.
    directory.close().map_err(|e| format!("remove worker copy: {e}"))?;
    Ok(status.code().and_then(|code| u8::try_from(code).ok()).unwrap_or(1))
}

fn build(root: &Path) -> Result<PathBuf, String> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut child = Command::new(cargo)
        .args([
            "build",
            "--locked",
            "--offline",
            "-p",
            "obc-data-steps",
            "--bin",
            "obc-data-worker",
            "--message-format=json",
        ])
        .current_dir(root)
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cargo build: {e}"))?;
    let mut executable = None;
    let messages = BufReader::new(child.stdout.take().unwrap());
    let parsed = (|| {
        for line in messages.lines() {
            let line = line.map_err(|e| format!("cargo build: {e}"))?;
            let message: serde_json::Value = serde_json::from_str(&line).map_err(|e| format!("cargo build: {e}"))?;
            if message["reason"] == "compiler-message" {
                if let Some(text) = message["message"]["rendered"].as_str() {
                    eprint!("{text}");
                }
            }
            if let Some(path) = artifact(&message) {
                executable = Some(path);
            }
        }
        Ok::<_, String>(())
    })();
    let status = child.wait().map_err(|e| format!("cargo build: {e}"))?;
    parsed?;
    if !status.success() {
        return Err("cargo could not build the producer worker; prepare the locked offline Rust dependencies".into());
    }
    executable.ok_or_else(|| "cargo named no producer worker executable".into())
}

fn artifact(message: &serde_json::Value) -> Option<PathBuf> {
    (message["reason"] == "compiler-artifact"
        && message["target"]["name"] == "obc-data-worker"
        && message["target"]["kind"].as_array()?.iter().any(|kind| kind == "bin"))
    .then(|| message["executable"].as_str().map(PathBuf::from))?
}

fn copy_worker(executable: &Path, directory: &Path) -> Result<PathBuf, String> {
    let copy = directory.join(format!("obc-data-worker{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(executable, &copy).map_err(|e| format!("copy {}: {e}", executable.display()))?;
    Ok(copy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_worker_binary_artifact_is_selected_and_its_copy_is_independent() {
        let mut message = serde_json::json!({"reason":"compiler-artifact", "target":{"name":"obc-data-worker", "kind":["bin"]}, "executable":"/target/worker"});
        assert_eq!(artifact(&message), Some(PathBuf::from("/target/worker")));
        message["target"]["kind"] = serde_json::json!(["lib"]);
        assert!(artifact(&message).is_none());
        message["target"]["kind"] = serde_json::json!(["bin"]);
        message["target"]["name"] = serde_json::json!("obc-data");
        assert!(artifact(&message).is_none());

        let source = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(source.path(), b"checked worker").unwrap();
        let directory = tempfile::tempdir().unwrap();
        let copy = copy_worker(source.path(), directory.path()).unwrap();
        std::fs::write(source.path(), b"new build").unwrap();
        assert_eq!(std::fs::read(&copy).unwrap(), b"checked worker");
        directory.close().unwrap();
        assert!(!copy.exists());
    }
}
