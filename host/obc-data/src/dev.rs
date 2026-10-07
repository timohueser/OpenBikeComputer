//! Finite Local preparation and stable ownership of the known planner services.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::engine::Code;
use crate::store::{hash_file, sha256_hex, Store};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Live,
    Local,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub region: Option<String>,
    pub source: Option<Source>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prepared {
    pub view: PathBuf,
    pub code: Code,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct State {
    pub token: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

fn directory(store: &Store) -> PathBuf {
    store.root().join("dev/local")
}

pub fn state(store: &Store) -> Result<Option<State>, String> {
    let path = directory(store).join("state.json");
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let mut state: State = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    if !store.is_locked("dev-local")? && state.status == "ready" {
        state.status = "interrupted".into();
    }
    Ok(Some(state))
}

pub fn stop(store: &Store) -> Result<Option<State>, String> {
    if let Some(state) = state(store)? {
        crate::commit::durable(
            &directory(store).join("stop.json"),
            &serde_json::to_vec(&serde_json::json!({
                "token": state.token
            }))
            .map_err(|error| error.to_string())?,
        )?;
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while store.is_locked("dev-local")? {
        if std::time::Instant::now() >= deadline {
            return Err("Local services are still draining; inspect their logs".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    state(store)
}

pub fn start(root: &Path, store: &Store, prepared: &Prepared) -> Result<State, String> {
    crate::worker::check(root)?;
    let environment = crate::operation::launch::preflight()?;
    let _admission = store.try_lock("dev-admission-local")?.ok_or("Local app admission is already in progress")?;
    let directory = directory(store);
    crate::commit::durable_directory(&directory)?;
    let token = if store.is_locked("dev-local")? {
        state(store)?.ok_or("Local owner has no readable state")?.token
    } else {
        sha256_hex(
            format!(
                "{}:{}",
                std::process::id(),
                SystemTime::now().duration_since(UNIX_EPOCH).map_err(|error| error.to_string())?.as_nanos()
            )
            .as_bytes(),
        )
    };
    let file = prepared.view.join("service.json");
    let digest = hash_file(&file)?.0;
    crate::commit::durable(
        &directory.join("desired.json"),
        &serde_json::to_vec(&serde_json::json!({
            "token": token, "view": prepared.view, "sha256": digest
        }))
        .map_err(|error| error.to_string())?,
    )?;
    if !store.is_locked("dev-local")? {
        crate::commit::durable(
            &directory.join("state.json"),
            &serde_json::to_vec(&State {
                token: token.clone(),
                status: "starting".into(),
                view: Some(prepared.view.clone()),
                url: None,
                message: None,
            })
            .map_err(|error| error.to_string())?,
        )?;
        let argv: Vec<String> = [
            "uv",
            "run",
            "--locked",
            "--offline",
            "--no-default-groups",
            "--no-sync",
            "--group",
            "search-runtime",
            "--no-python-downloads",
            "python",
            "-m",
            "tools.planner_local",
            "--directory",
        ]
        .map(String::from)
        .into();
        let mut command = prepared.code.command(root, &argv)?;
        command.arg(&directory).args(["--token", &token, "--lock"]).arg(store.lock_path("dev-local"));
        #[cfg(target_os = "macos")]
        {
            crate::operation::launch::mac(&mut command, &directory)?;
            command.spawn().map_err(|error| error.to_string())?;
        }
        #[cfg(target_os = "linux")]
        {
            let file = environment.ok_or("Local services need the configured private environment")?;
            let mut launch =
                crate::operation::launch::service(&format!("obc-data-dev-{token}"), root, &directory, &file, true);
            launch.arg("env");
            for (key, value) in command.get_envs() {
                if let Some(value) = value {
                    let mut assignment = key.to_os_string();
                    assignment.push("=");
                    assignment.push(value);
                    launch.arg(assignment);
                }
            }
            launch.arg(command.get_program()).args(command.get_args());
            if !launch.status().map_err(|error| error.to_string())?.success() {
                return Err("Local service admission failed".into());
            }
        }
        #[cfg(not(target_os = "linux"))]
        let _ = environment;
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(100);
    loop {
        if let Some(state) = state(store)? {
            if state.token != token {
                return Err("Local owner changed during startup".into());
            }
            if state.status == "ready" && state.view.as_ref() == Some(&prepared.view) {
                prepared.code.files(root)?;
                return Ok(state);
            }
            if matches!(state.status.as_str(), "failed" | "stopped" | "interrupted") {
                return Err(state.message.unwrap_or_else(|| format!("Local services are {}", state.status)));
            }
        }
        if std::time::Instant::now() >= deadline {
            return Err("Local services did not reach readiness; inspect their logs".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

pub fn open(store: &Store) -> Result<(), String> {
    let state = state(store)?.ok_or("Local Web planner is not running")?;
    if state.status != "ready" {
        return Err("Local Web planner is not ready".into());
    }
    let url = state.url.ok_or("Local Web planner has no checked URL")?;
    if url != "http://127.0.0.1:5173/planner.html" {
        return Err("Local Web planner URL differs from the known loopback service".into());
    }
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(not(target_os = "macos"))]
    let program = "xdg-open";
    if !Command::new(program).arg(url).status().map_err(|error| error.to_string())?.success() {
        return Err("Cannot open the Local Web planner in the browser".into());
    }
    Ok(())
}
