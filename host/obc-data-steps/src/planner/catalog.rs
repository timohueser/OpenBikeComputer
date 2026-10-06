//! Planner publication metadata and verification from stored receipts.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use obc_data::engine::release::Release;
use obc_data::engine::LayerFile;
use obc_data::product::Pointer;
use obc_data::store::{hash_file, Store};
use serde::Deserialize;
use serde_json::{json, Value};

const INDEX: &str = "planner/index";
const PUBLIC: &str = "https://maps.openbikecomputer.com/planner";
const TILES: &str = "https://tiles.openbikecomputer.com";

pub fn named(release: &Release) -> Result<Vec<LayerFile>, String> {
    let index = release.layers.iter().find(|layer| layer.step == INDEX).ok_or("release has no planner index")?;
    let mut files: Vec<_> = index
        .files
        .iter()
        .filter(|file| file.path == "release.json" || file.path.starts_with("public/"))
        .cloned()
        .collect();
    for layer in release.layers.iter().filter(|layer| layer.step.ends_with("/grid") && !layer.client.is_none()) {
        let mut file =
            layer.files.iter().find(|file| file.path == "index.json").ok_or("planner grid has no index")?.clone();
        file.path = format!("indexes/{}/index.json", layer.step);
        files.push(file);
    }
    Ok(files)
}

pub fn pointer(release: &Release, store: &Store) -> Result<Pointer, String> {
    let document: Value = serde_json::from_slice(&descriptor(release, store)?).map_err(|e| e.to_string())?;
    let id = release.id();
    let tiles = format!("{TILES}/releases/{id}");
    let files = document["files"].as_object().ok_or("planner index has no files")?;
    let layers: BTreeMap<_, _> = ["snow", "climate", "sun"]
        .into_iter()
        .filter(|kind| files.contains_key(&format!("maps/{kind}.json")))
        .map(|kind| (kind, format!("{tiles}/{kind}.json")))
        .collect();
    let mut active = json!({
        "id": id, "manifest": format!("{PUBLIC}/releases/{id}/release.json"),
        "region": document["region"], "name": document["name"], "bounds": document["bounds"],
        "basemap": format!("{tiles}/basemap.json"), "places": format!("{tiles}/places.json"),
        "overlays": format!("{tiles}/overlays.json"), "terrain": format!("{tiles}/terrain/{{z}}/{{x}}/{{y}}.webp"),
        "glyphs": format!("{tiles}/maps/assets/fonts/{{fontstack}}/{{range}}.pbf"),
        "sprites": format!("{tiles}/maps/assets/sprites/v4"), "routes": format!("{tiles}/routes/tiles/{{cell}}.json"),
        "attribution": document["attribution"], "landcover_attribution": document["landcover_attribution"],
        "terrain_attribution": document["terrain_attribution"], "layers": layers,
    });
    let services = super::runtime::identities(release)?;
    if services.as_object().is_some_and(|services| !services.is_empty()) {
        active["services"] = services;
    }
    // The required runtime layer blocks publication until real code receipts supply service endpoints.
    Ok(Pointer { document: json!({"format": 1, "active": active}).as_object().unwrap().clone() })
}

fn checked(store: &Store, file: &LayerFile) -> Result<std::path::PathBuf, String> {
    let path = store.object(&file.sha256);
    if hash_file(&path)? != (file.sha256.clone(), file.size) {
        return Err(format!("planner artifact `{}` differs from its receipt", file.path));
    }
    Ok(path)
}

fn descriptor(release: &Release, store: &Store) -> Result<Vec<u8>, String> {
    let file =
        release.named.iter().find(|file| file.path == "release.json").ok_or("planner has no named release.json")?;
    std::fs::read(checked(store, file)?).map_err(|e| e.to_string())
}

#[derive(Deserialize)]
struct Stored {
    bytes: u64,
    sha256: String,
}

#[derive(Deserialize)]
struct File {
    transport: Stored,
}

pub fn verify(root: &Path, previous: Option<&Release>, release: &Release, store: &Store) -> Result<(), String> {
    release.check_named()?;
    super::runtime::verify(previous, release, store)?;
    let body = descriptor(release, store)?;
    let document: Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    if release.product != "planner"
        || document["region"] != release.region.rsplit('/').next().unwrap_or("")
        || document["format"] != 1
        || document["grid"] != json!({"format": 2, "zoom": 9, "map_zoom": 11})
    {
        return Err("planner region or grid format differs".into());
    }
    let previous: Option<Value> = previous
        .map(|release| {
            descriptor(release, store).and_then(|body| serde_json::from_slice(&body).map_err(|e| e.to_string()))
        })
        .transpose()?;
    let changed = |name: &str| previous.as_ref().is_none_or(|old| old["files"][name] != document["files"][name]);
    let routing_changed = document["files"]
        .as_object()
        .ok_or("planner index has no files")?
        .keys()
        .any(|name| name.starts_with("routing/") && changed(name));
    let files: BTreeMap<String, File> = serde_json::from_value(document["files"].clone()).map_err(|e| e.to_string())?;
    let owned = release.objects();
    let mut paths = BTreeMap::new();
    for (name, file) in files {
        let stored = file.transport;
        if stored.sha256.len() != 64
            || !stored.sha256.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            || owned.get(stored.sha256.as_str()) != Some(&stored.bytes)
        {
            return Err(format!("planner file `{name}` is not a client object of this release"));
        }
        if changed(&name) || name.ends_with(".json") || routing_changed && name.starts_with("routing/") {
            let file =
                LayerFile { path: format!("objects/{}", stored.sha256), size: stored.bytes, sha256: stored.sha256 };
            paths.insert(file.path.clone(), checked(store, &file)?);
        }
    }
    for file in &release.named {
        paths.insert(file.path.clone(), checked(store, file)?);
    }
    let scratch = tempfile::tempdir_in(store.root()).map_err(|e| e.to_string())?;
    let source = scratch.path().join("source");
    for (name, path) in paths {
        let target = source.join(name);
        std::fs::create_dir_all(target.parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::hard_link(path, target).map_err(|e| e.to_string())?;
    }
    if let Some(previous) = previous {
        std::fs::write(source.join("previous.json"), serde_json::to_vec(&previous).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    }
    let ready = Command::new("uv")
        .args([
            "sync",
            "--locked",
            "--offline",
            "--check",
            "--inexact",
            "--no-default-groups",
            "--no-python-downloads",
            "--group",
            "planner-maps",
        ])
        .current_dir(root)
        .output()
        .map_err(|e| format!("planner verification needs the offline uv tooling: {e}"))?;
    if !ready.status.success() {
        return Err(format!(
            "prepare the locked planner-maps tooling before verification: {}",
            String::from_utf8_lossy(&ready.stderr).trim()
        ));
    }
    let output = Command::new("uv")
        .args([
            "run",
            "--locked",
            "--offline",
            "--no-sync",
            "--no-default-groups",
            "--no-python-downloads",
            "--group",
            "planner-maps",
            "python",
            "-m",
            "tools.planner_verify",
        ])
        .arg(&source)
        .current_dir(root)
        .output()
        .map_err(|e| format!("planner verification needs the offline uv tooling: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "planner stored-artifact verification failed; prepare the locked planner-maps tooling first: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    if routing_changed {
        route_engine::open(&source.join("runtime/routing"))
            .and_then(|graph| graph.verify())
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
