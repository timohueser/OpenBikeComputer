//! Finite Local preparation and stable ownership of the known planner services.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub mod apps;
pub use apps::{App, AppState};

use crate::engine::Code;
use crate::store::{hash_file, sha256_hex, Store};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub region: Option<String>,
    pub refresh_live: bool,
    pub app: App,
    pub inputs_only: bool,
    pub reviewed: Option<Box<crate::cli::EnvPlan>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Prepared {
    pub view: PathBuf,
    pub descriptor: String,
    pub supervisor: Binding,
    pub children: std::collections::BTreeMap<String, Binding>,
    pub apps: std::collections::BTreeSet<App>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub code: Code,
    pub files: std::collections::BTreeMap<String, String>,
}

impl Binding {
    pub fn check(&self, root: &Path) -> Result<(), String> {
        if self.code.files(root)? != self.files {
            return Err("Local execution providers changed; prepare the Local view again".into());
        }
        Ok(())
    }
}

impl Prepared {
    pub fn check(&self, root: &Path) -> Result<(), String> {
        self.check_apps(root, &self.apps)
    }

    pub fn check_apps(&self, root: &Path, apps: &std::collections::BTreeSet<App>) -> Result<(), String> {
        if !apps.is_subset(&self.apps) {
            return Err("Prepare the selected Local app first".into());
        }
        let body = std::fs::read(self.view.join("service.json")).map_err(|e| e.to_string())?;
        if sha256_hex(&body) != self.descriptor {
            return Err("Prepared Local service view changed".into());
        }
        let value: serde_json::Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
        if value["root"] != root.canonicalize().map_err(|e| e.to_string())?.to_str().ok_or("Local root is not UTF-8")?
            || value["view"] != self.view.to_str().ok_or("Local view is not UTF-8")?
        {
            return Err("Prepared Local service view belongs to another root".into());
        }
        let (env, _) = crate::env::Env::local(root, &crate::regions::Regions::load(root)?, None)?;
        if value["region"] != env.region
            || value["layers"] != serde_json::json!(env.layers)
            || value["configuration"] != configuration(root)?
        {
            return Err("Prepare the selected Local region and layers before starting this app".into());
        }
        self.supervisor.check(root)?;
        for name in apps.iter().flat_map(|app| app.children()).collect::<std::collections::BTreeSet<_>>() {
            self.children.get(*name).ok_or_else(|| format!("Prepare the Local {name} service first"))?.check(root)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct State {
    pub token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    pub status: String,
    pub apps: std::collections::BTreeMap<App, AppState>,
    pub region: Option<String>,
    pub layers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Bind the selected region definition and layers, including edits under an unchanged region id.
pub fn configuration(root: &Path) -> Result<String, String> {
    let regions = crate::regions::Regions::load(root)?;
    let (env, _) = crate::env::Env::local(root, &regions, None)?;
    configuration_for(&env, &regions)
}

/// The exact semantic configuration captured for one preparation.
pub fn configuration_for(env: &crate::env::Env, regions: &crate::regions::Regions) -> Result<String, String> {
    let leaves: Vec<_> = regions.leaves(&env.region)?.into_iter().map(|id| regions.get(id)).collect();
    Ok(sha256_hex(&serde_json::to_vec(&(regions.get(&env.region), leaves, &env.layers)).map_err(|e| e.to_string())?))
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
        for app in state.apps.values_mut().filter(|app| matches!(app.status.as_str(), "ready" | "starting")) {
            app.status = "interrupted".into();
        }
    }
    Ok(Some(state))
}

pub fn stop(store: &Store) -> Result<Option<State>, String> {
    let _admission = store.try_lock("dev-admission-local")?.ok_or("Local app admission is already in progress")?;
    let state = stop_owner(store)?;
    let safe = match &state {
        Some(state) => state.status == "stopped" || drained(store, state)?,
        None => !directory(store).join("desired.json").exists(),
    };
    if safe {
        clean_views(store)?;
    }
    Ok(state)
}

fn stop_owner(store: &Store) -> Result<Option<State>, String> {
    if let Some(state) = state(store)? {
        crate::store::durable(
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

fn same_ready_owner(store: &Store, original: &State) -> Result<bool, String> {
    Ok(store.is_locked("dev-local")?
        && state(store)?.is_some_and(|current| current.token == original.token && current.status == "ready"))
}

fn drained(store: &Store, state: &State) -> Result<bool, String> {
    let bytes = match std::fs::read(directory(store).join("drained.json")) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.to_string()),
    };
    let proof: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    Ok(proof["token"] == state.token)
}

pub fn start(root: &Path, store: &Store, prepared: &Prepared, app: App) -> Result<State, String> {
    change(root, store, prepared, Some(app))?.ok_or_else(|| "Local start did not admit an owner".into())
}

/// A completed preparation updates only an already running owner.
pub fn replace(root: &Path, store: &Store, prepared: &Prepared) -> Result<Option<State>, String> {
    change(root, store, prepared, None)
}

fn change(root: &Path, store: &Store, prepared: &Prepared, start: Option<App>) -> Result<Option<State>, String> {
    let _admission = store.try_lock("dev-admission-local")?.ok_or("Local app admission is already in progress")?;
    let start_stopped = start.is_some();
    let original = state(store)?;
    let mut apps: std::collections::BTreeSet<_> = original
        .as_ref()
        .into_iter()
        .flat_map(|state| &state.apps)
        .filter(|(_, state)| state.status == "ready")
        .map(|(app, _)| *app)
        .collect();
    apps.extend(start);
    let updating = original.as_ref().filter(|state| state.status == "ready");
    if !start_stopped && (!store.is_locked("dev-local")? || updating.is_none()) {
        return Ok(None);
    }
    crate::worker::check(root)?;
    if !prepared
        .view
        .canonicalize()
        .map_err(|e| e.to_string())?
        .starts_with(directory(store).join("views").canonicalize().map_err(|e| e.to_string())?)
    {
        return Err("Local service view leaves its store".into());
    }
    prepared.check_apps(root, &apps)?;
    let code =
        crate::engine::digest(prepared.supervisor.files.iter().map(|(key, value)| (key.as_str(), value.as_str())));
    let directory = directory(store);
    crate::store::durable_directory(&directory)?;
    if !start_stopped && !same_ready_owner(store, updating.expect("checked above"))? {
        return Ok(None);
    }
    let mut replaced = false;
    if store.is_locked("dev-local")? && state(store)?.is_some_and(|state| state.code.as_ref() != Some(&code)) {
        let stopped = stop_owner(store)?;
        if !start_stopped {
            let Some(state) = stopped else { return Ok(None) };
            if state.token != updating.expect("checked above").token || !drained(store, &state)? {
                return Ok(None);
            }
        }
        replaced = true;
    }
    if !store.is_locked("dev-local")? && state(store)?.is_some_and(|state| state.status == "starting") {
        let stopped = std::fs::read(directory.join("stop.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .is_some_and(|stop| state(store).ok().flatten().is_some_and(|state| stop["token"] == state.token));
        if !stopped {
            return Err("Local owner is still being admitted; stop it before another start".into());
        }
    }
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
    if digest != prepared.descriptor {
        return Err("Prepared Local descriptor changed during admission".into());
    }
    if !start_stopped && !replaced && !same_ready_owner(store, updating.expect("checked above"))? {
        return Ok(None);
    }
    let mut intent: std::collections::BTreeMap<App, String> = match std::fs::read(directory.join("desired.json")) {
        Ok(bytes) => {
            let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            serde_json::from_value(value["apps"].clone()).map_err(|e| e.to_string())?
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Default::default(),
        Err(error) => return Err(error.to_string()),
    };
    intent.retain(|app, _| apps.contains(app));
    if let Some(app) = start {
        intent.insert(
            app,
            SystemTime::now().duration_since(UNIX_EPOCH).map_err(|e| e.to_string())?.as_nanos().to_string(),
        );
    }
    crate::store::durable(
        &directory.join("desired.json"),
        &serde_json::to_vec(&serde_json::json!({
            "token": token, "view": prepared.view, "sha256": digest, "code": code, "apps": intent
        }))
        .map_err(|error| error.to_string())?,
    )?;
    if !store.is_locked("dev-local")? {
        if !start_stopped && !replaced {
            return Ok(None);
        }
        crate::store::durable(
            &directory.join("state.json"),
            &serde_json::to_vec(&State {
                token: token.clone(),
                code: Some(code.clone()),
                status: "starting".into(),
                apps: Default::default(),
                region: None,
                layers: Vec::new(),
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
            "--no-python-downloads",
            "python",
            "-m",
            "tools.planner_local",
            "--directory",
        ]
        .map(String::from)
        .into();
        let mut command = prepared.supervisor.code.command(root, &argv)?;
        let python = command
            .get_envs()
            .find(|(name, _)| *name == "UV_PYTHON")
            .and_then(|(_, value)| value.map(std::ffi::OsStr::to_os_string))
            .ok_or("Local supervisor has no selected Python")?;
        command
            .arg(&directory)
            .args(["--token", &token, "--lock"])
            .arg(store.lock_path("dev-local"))
            .arg("--base-python")
            .arg(python)
            .arg("--python-runtime")
            .arg(
                prepared
                    .supervisor
                    .files
                    .get("python/runtime")
                    .ok_or("Local supervisor has no interpreter identity")?,
            );
        crate::operation::launch::detach(&mut command, &directory)?;
        command.spawn().map_err(|error| error.to_string())?;
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(100);
    loop {
        if let Some(state) = state(store)? {
            if state.token != token {
                return Err("Local owner changed during startup".into());
            }
            if state.status == "ready"
                && state.view.as_ref() == Some(&prepared.view)
                && apps.iter().all(|app| state.apps.get(app).is_some_and(|state| state.status == "ready"))
            {
                if let Err(error) = prepared.check_apps(root, &apps) {
                    stop_owner(store)?;
                    return Err(error);
                }
                return Ok(Some(state));
            }
            if let Some(failed) = start.and_then(|app| state.apps.get(&app)).filter(|state| state.status == "failed") {
                return Err(failed.message.clone().unwrap_or_else(|| "Local app failed".into()));
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

fn clean_views(store: &Store) -> Result<(), String> {
    let Some(_control) = store.try_lock("operation-control-local")? else { return Ok(()) };
    let Some(_preparing) = store.try_lock("operation-active-local")? else { return Ok(()) };
    match std::fs::read_to_string(crate::operation::active_path(store, "local")) {
        Ok(run) => {
            if !crate::operation::read(store, &run)?.is_some_and(|control| control.state.terminal()) {
                return Ok(());
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    let kept = read_prepared(store)?.map(|prepared| prepared.view);
    let views = directory(store).join("views");
    let metadata = match std::fs::symlink_metadata(&views) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    if metadata.file_type().is_symlink() {
        return Err("Local views directory must not be a symlink".into());
    }
    for entry in std::fs::read_dir(&views).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !entry.file_type().map_err(|e| e.to_string())?.is_dir() || kept.as_ref() == Some(&entry.path()) {
            continue;
        }
        if let Some(run) = name.strip_prefix('.') {
            if crate::engine::runs::check_id(run).is_err()
                || !crate::operation::read(store, run)?.is_some_and(|control| {
                    control.request.kind == crate::operation::Kind::DevPrepare && control.state.terminal()
                })
            {
                continue;
            }
        } else {
            if name.len() != 64 || !name.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
                continue;
            }
            let descriptor = std::fs::read(entry.path().join("service.json")).map_err(|e| e.to_string())?;
            let value: serde_json::Value = serde_json::from_slice(&descriptor).map_err(|e| e.to_string())?;
            if value["view"].as_str() != entry.path().to_str() {
                return Err("Obsolete Local view has no matching ownership descriptor".into());
            }
        }
        std::fs::remove_dir_all(entry.path()).map_err(|e| e.to_string())?;
    }
    crate::store::durable_directory(&views)
}

pub fn open(store: &Store, app: App) -> Result<(), String> {
    let state = state(store)?.ok_or("Local Web planner is not running")?;
    if state.status != "ready" {
        return Err("Local Web planner is not ready".into());
    }
    if state.apps.get(&app).is_none_or(|state| state.status != "ready") {
        return Err(format!("{} is not ready", app.name()));
    }
    let url = app.url().ok_or("Simulator has no browser address")?;
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(not(target_os = "macos"))]
    let program = "xdg-open";
    if !Command::new(program).arg(url).status().map_err(|error| error.to_string())?.success() {
        return Err("Cannot open the Local Web planner in the browser".into());
    }
    Ok(())
}

/// Recent bounded supervisor output; reading logs does not change its state.
pub fn logs(store: &Store, app: App) -> Result<Vec<String>, String> {
    let mut logs = crate::cli::operation_cli::tail(&directory(store).join("stderr.log"))?;
    for child in app.children() {
        logs.extend(
            crate::cli::operation_cli::tail(&directory(store).join(format!("{child}.log")))?
                .into_iter()
                .map(|line| format!("{child}: {line}")),
        );
    }
    Ok(logs)
}

/// Stop only children that no other requested app needs.
pub fn stop_app(store: &Store, app: App) -> Result<Option<State>, String> {
    let _admission = store.try_lock("dev-admission-local")?.ok_or("Local app admission is already in progress")?;
    let Some(owner) = state(store)? else { return Ok(None) };
    if !store.is_locked("dev-local")? {
        if owner.status == "stopped" || drained(store, &owner)? {
            clean_views(store)?;
        }
        return Ok(Some(owner));
    }
    let file = directory(store).join("desired.json");
    let mut desired: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&file).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    if desired["token"] != owner.token {
        return Err("Local owner changed before stop".into());
    }
    let mut apps: std::collections::BTreeMap<App, String> =
        serde_json::from_value(desired["apps"].clone()).map_err(|e| e.to_string())?;
    apps.remove(&app);
    if apps.is_empty() {
        let stopped = stop_owner(store)?;
        if stopped.as_ref().is_some_and(|state| state.status == "stopped")
            || stopped.as_ref().map(|state| drained(store, state)).transpose()?.unwrap_or(false)
        {
            clean_views(store)?;
        }
        return Ok(stopped);
    }
    desired["apps"] = serde_json::to_value(apps).map_err(|e| e.to_string())?;
    crate::store::durable(&file, &serde_json::to_vec(&desired).map_err(|e| e.to_string())?)?;
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let current = state(store)?;
        if current.as_ref().is_none_or(|state| state.token != owner.token) {
            return Err("Local owner changed while stopping an app".into());
        }
        if current.as_ref().is_some_and(|state| state.apps.get(&app).is_none_or(|app| app.status == "stopped")) {
            return Ok(current);
        }
        if std::time::Instant::now() >= deadline {
            return Err("Local app is still draining; inspect its logs".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Read the last completed preparation without starting any producer or service.
pub fn prepared(store: &Store) -> Result<Prepared, String> {
    read_prepared(store)?.ok_or_else(|| "Prepare Local data first".into())
}

fn read_prepared(store: &Store) -> Result<Option<Prepared>, String> {
    match std::fs::read(directory(store).join("prepared.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|e| e.to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct Observed {
    pub state: Option<State>,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct Logs {
    pub logs: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_app_admits_only_its_providers_and_rejects_pending_region_changes() {
        let scratch = crate::store::tests::Scratch::new("dev-app-providers");
        let root = scratch.0.canonicalize().unwrap();
        for dir in ["data/env", "data/regions", "view"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        assert!(Command::new("git").args(["init", "-q"]).current_dir(&root).status().unwrap().success());
        let region = root.join("data/regions/ride.toml");
        std::fs::write(
            &region,
            "name='Ride'\nkind='box'\nbox=[7,47,8,48]\ncountries=['DE']\ntime_zone='Europe/Berlin'\n",
        )
        .unwrap();
        std::fs::write(crate::env::Env::path(&root, "local"), "region='ride'\nlayers=[]\n").unwrap();
        let binary = root.join("view/obc-sim");
        std::fs::write(&binary, b"retained Simulator").unwrap();
        let code = Code {
            libraries: vec![crate::engine::Library {
                version: None,
                name: "simulator".into(),
                path: binary.clone(),
                sha256: hash_file(&binary).unwrap().0,
            }],
            ..Default::default()
        };
        let simulator = Binding { files: code.files(&root).unwrap(), code };
        let code = Code::default();
        let supervisor = Binding { files: code.files(&root).unwrap(), code };
        let missing = Binding {
            code: Code {
                libraries: vec![crate::engine::Library {
                    version: None,
                    name: "node".into(),
                    path: root.join("absent-node"),
                    sha256: "0".repeat(64),
                }],
                ..Default::default()
            },
            files: Default::default(),
        };
        let body = serde_json::to_vec(&serde_json::json!({"root":root,"view":root.join("view"),"region":"ride","layers":[],"configuration":configuration(&root).unwrap()})).unwrap();
        std::fs::write(root.join("view/service.json"), &body).unwrap();
        let prepared = Prepared {
            view: root.join("view"),
            descriptor: sha256_hex(&body),
            supervisor,
            children: [("simulator".into(), simulator), ("routing".into(), missing)].into(),
            apps: App::ALL.into(),
        };
        prepared.check_apps(&root, &[App::Simulator].into()).unwrap();
        assert!(
            prepared.check_apps(&root, &[App::WebPlanner].into()).is_err(),
            "an absent Web provider cannot admit Web"
        );
        let captured_regions = crate::regions::Regions::load(&root).unwrap();
        let (captured_env, _) = crate::env::Env::local(&root, &captured_regions, None).unwrap();
        let captured = configuration_for(&captured_env, &captured_regions).unwrap();
        std::fs::write(
            &region,
            "name='Ride'\nkind='box'\nbox=[7,47,7.5,48]\ncountries=['DE']\ntime_zone='Europe/Berlin'\n",
        )
        .unwrap();
        assert_eq!(
            captured,
            configuration_for(&captured_env, &captured_regions).unwrap(),
            "preparation keeps its original configuration snapshot"
        );
        assert_ne!(captured, configuration(&root).unwrap());
        assert!(
            prepared.check_apps(&root, &[App::Simulator].into()).unwrap_err().contains("selected Local region"),
            "the saved id cannot hide changed geometry"
        );
    }

    #[test]
    fn preparation_leaves_stopped_apps_and_cleanup_preserves_current_or_busy_views() {
        let scratch = crate::store::tests::Scratch::new("dev-stopped-views");
        let store = Store::at(&scratch.0);
        let views = directory(&store).join("views");
        let current = views.join("a".repeat(64));
        let obsolete = views.join("b".repeat(64));
        for view in [&current, &obsolete] {
            crate::store::durable(
                &view.join("service.json"),
                &serde_json::to_vec(&serde_json::json!({"view":view})).unwrap(),
            )
            .unwrap();
            std::fs::write(view.join("planner-service"), b"retained native bytes").unwrap();
        }
        let binding = Binding { code: Code::default(), files: Default::default() };
        let prepared = Prepared {
            view: current.clone(),
            descriptor: "unused".into(),
            supervisor: binding,
            children: Default::default(),
            apps: Default::default(),
        };
        crate::store::durable(&directory(&store).join("prepared.json"), &serde_json::to_vec(&prepared).unwrap())
            .unwrap();
        assert!(replace(Path::new("/absent-checkout"), &store, &prepared).unwrap().is_none());
        assert!(state(&store).unwrap().is_none(), "preparation does not admit a stopped app owner");
        let stopped = State {
            token: "owner".into(),
            code: None,
            status: "stopped".into(),
            apps: Default::default(),
            region: None,
            layers: Vec::new(),
            view: Some(obsolete.clone()),
            url: None,
            message: None,
        };
        crate::store::durable(&directory(&store).join("state.json"), &serde_json::to_vec(&stopped).unwrap()).unwrap();
        let preparing = store.lock("operation-active-local").unwrap();
        stop(&store).unwrap();
        assert!(obsolete.join("planner-service").exists(), "an active preparation prevents view cleanup");
        drop(preparing);
        stop(&store).unwrap();
        assert!(!obsolete.exists());
        assert!(current.join("planner-service").exists());
        crate::store::durable(
            &obsolete.join("service.json"),
            &serde_json::to_vec(&serde_json::json!({"view":obsolete})).unwrap(),
        )
        .unwrap();
        std::fs::write(obsolete.join("planner-service"), b"retained native bytes").unwrap();
        let mut uncertain = stopped;
        uncertain.status = "starting".into();
        crate::store::durable(&directory(&store).join("state.json"), &serde_json::to_vec(&uncertain).unwrap()).unwrap();
        stop(&store).unwrap();
        assert!(obsolete.exists(), "a delayed or uncertain owner is not a cleanup proof");
        uncertain.status = "failed".into();
        uncertain.message = Some("service failure".into());
        crate::store::durable(&directory(&store).join("state.json"), &serde_json::to_vec(&uncertain).unwrap()).unwrap();
        stop(&store).unwrap();
        assert!(obsolete.exists(), "failure alone does not prove that children drained");
        crate::store::durable(&directory(&store).join("drained.json"), br#"{"token":"another-owner"}"#).unwrap();
        stop(&store).unwrap();
        assert!(obsolete.exists(), "a different owner's drain proof is insufficient");
        crate::store::durable(&directory(&store).join("drained.json"), br#"{"token":"owner"}"#).unwrap();
        let failed = stop(&store).unwrap().unwrap();
        assert_eq!(failed.status, "failed");
        assert_eq!(failed.message.as_deref(), Some("service failure"));
        assert!(!obsolete.exists());
        let owner = store.lock("dev-local").unwrap();
        uncertain.status = "ready".into();
        crate::store::durable(&directory(&store).join("state.json"), &serde_json::to_vec(&uncertain).unwrap()).unwrap();
        assert!(same_ready_owner(&store, &uncertain).unwrap());
        drop(owner);
        assert!(!same_ready_owner(&store, &uncertain).unwrap(), "a lost ready owner cannot be replaced");
    }
}
