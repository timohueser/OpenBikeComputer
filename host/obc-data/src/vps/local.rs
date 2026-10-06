//! The known planner helper runs from checked artifact bytes, without a deployed checkout.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use super::{Candidate, Installed, Ready, Service, Stage, State, Vps};
use crate::r2::{Bucket, Scratch};
use crate::store::{hash_file, Store};

pub(crate) struct Local<'a> {
    store: &'a Store,
    bucket: &'a Bucket,
    helper: Scratch,
    api_origin: String,
    views: Vec<Scratch>,
}

fn checked(path: &Path, sha: &str, size: u64) -> Result<(), String> {
    if hash_file(path)? != (sha.to_string(), size) {
        return Err("planner installation input differs from its receipt".into());
    }
    Ok(())
}

fn metadata(candidate: &Candidate) -> Result<(Value, Value), String> {
    let read = |file: &crate::engine::LayerFile| {
        let path = candidate.source.join(&file.path);
        checked(&path, &file.sha256, file.size)?;
        serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
    };
    Ok((read(&candidate.release)?, read(&candidate.runtime)?))
}

impl<'a> Local<'a> {
    pub fn new(store: &'a Store, bucket: &'a Bucket, downloads: &Candidate) -> Result<Self, String> {
        let helper = Scratch::new()?;
        let mut local = Self { store, bucket, helper, api_origin: downloads.api_origin.clone(), views: Vec::new() };
        let (_, descriptor) = metadata(downloads)?;
        let archive = local.object(&descriptor["payload"])?;
        let python = downloads.target.python.as_deref().ok_or("downloads runtime has no configured CPython")?;
        let output = Command::new("python3")
            .args(["-S", "-c", include_str!("bootstrap.py")])
            .arg(archive)
            .arg(descriptor["payload"]["sha256"].as_str().ok_or("runtime payload has no SHA-256")?)
            .arg(&local.helper.0)
            .arg(python)
            .output()
            .map_err(|e| format!("prepare the exact offline CPython on the owner: {e}"))?;
        if !output.status.success() {
            return Err(format!("installer bootstrap: {}", String::from_utf8_lossy(&output.stderr).trim()));
        }
        Ok(local)
    }

    fn object(&mut self, item: &Value) -> Result<PathBuf, String> {
        let sha = item["sha256"].as_str().ok_or("service input has no SHA-256")?;
        let size = item["bytes"].as_u64().ok_or("service input has no size")?;
        if sha.len() != 64 || !sha.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)) {
            return Err("service input has an invalid SHA-256".into());
        }
        let path = self.store.object(sha);
        if !path.is_file() {
            let temporary = self.helper.0.join(format!("object-{sha}"));
            self.bucket.get(&format!("planner/objects/{sha}"), &temporary)?;
            checked(&temporary, sha, size)?;
            self.store.insert(&temporary, sha)?;
        }
        checked(&path, sha, size)?;
        Ok(path)
    }

    fn call<T: DeserializeOwned>(&mut self, command: &str, request: &Value) -> Result<T, String> {
        let mut child = Command::new("python3");
        child
            .args(["-S", "-m", "tools.planner_install", command])
            .env("PYTHONPATH", &self.helper.0)
            .current_dir(&self.helper.0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if command == "inspect" {
            child.args(["--api-origin", &self.api_origin]);
        }
        let mut child = child.spawn().map_err(|e| e.to_string())?;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(request).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        let output = child.wait_with_output().map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err(format!("planner {command}: {}", String::from_utf8_lossy(&output.stderr).trim()));
        }
        serde_json::from_slice(&output.stdout).map_err(|e| format!("planner {command} response: {e}"))
    }
}

impl Vps for Local<'_> {
    fn inspect(&mut self) -> Result<State, String> {
        self.call("inspect", &Value::Null)
    }

    fn prepare(&mut self, staged: &Stage) -> Result<Stage, String> {
        let (release, runtime) = metadata(&staged.candidate)?;
        let source = Scratch::new()?;
        std::fs::create_dir(source.0.join("objects")).map_err(|e| e.to_string())?;
        let mut prepared = staged.clone();
        for file in [&staged.candidate.release, &staged.candidate.runtime] {
            let target = source.0.join(&file.path);
            std::fs::create_dir_all(target.parent().unwrap()).map_err(|e| e.to_string())?;
            std::fs::hard_link(staged.candidate.source.join(&file.path), target).map_err(|e| e.to_string())?;
        }
        let prefix = match staged.installed.service {
            Service::Routing => "routing/",
            Service::Search => "search/",
            Service::Downloads => "offline/",
        };
        let mut objects = vec![runtime["payload"].clone()];
        for (name, file) in release["files"].as_object().ok_or("service release has no files")? {
            if name.starts_with(prefix) {
                objects.push(file["transport"].clone());
            }
        }
        for item in objects {
            let path = self.object(&item)?;
            let target = source.0.join("objects").join(item["sha256"].as_str().unwrap());
            if !target.exists() {
                std::fs::hard_link(path, target).map_err(|e| e.to_string())?;
            }
        }
        prepared.candidate.source = source.0.clone();
        self.views.push(source);
        Ok(prepared)
    }

    fn stage(&mut self, staged: &Stage) -> Result<(), String> {
        self.call("stage", &json!(staged))
    }
    fn probe(&mut self, staged: &Stage) -> Result<Ready, String> {
        self.call("probe", &json!(staged))
    }
    fn activate(&mut self, stages: &[Stage]) -> Result<Vec<Ready>, String> {
        self.call("activate", &json!({"stages": stages}))
    }
    fn retire(&mut self, previous: &[Installed], current: &[Installed]) -> Result<(), String> {
        self.call("retire", &json!({"previous": previous, "current": current}))
    }
}
