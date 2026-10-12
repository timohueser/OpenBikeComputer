//! The pointer of the planner, the files that its clients find by name, and the check of a
//! release before a target serves it.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use obc_data::engine::{LayerFile, Request};
use obc_data::release::Release;
use obc_data::store::{hash_file, Store};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const INDEX: &str = "planner/index";
/// The step that writes the pointer.
pub const STEP: &str = "planner/catalog";

#[derive(Deserialize)]
struct Config {
    publication: Origins,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Origins {
    pub site_origin: String,
    pub api_origin: String,
    pub objects_origin: String,
    /// The tile Worker, which reads `releases/<id>/public/` and `objects/` of the bucket.
    pub tiles_origin: String,
}

/// The origins of `data/planner-runtime.toml` that the pointer names.
pub(super) fn publication(root: &Path) -> Result<Origins, String> {
    let path = root.join("data/planner-runtime.toml");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let config: Config = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let publication = &config.publication;
    for origin in
        [&publication.site_origin, &publication.api_origin, &publication.objects_origin, &publication.tiles_origin]
    {
        let host = origin.strip_prefix("https://").ok_or("publication needs HTTPS origins")?;
        if host.is_empty() || !host.bytes().all(|byte| byte.is_ascii_alphanumeric() || b".-:".contains(&byte)) {
            return Err(format!("publication origin `{origin}` has a path, credentials or unsafe characters"));
        }
    }
    Ok(config.publication)
}

/// The files that a planner client finds by name, from the files of each step: the release
/// document and the public files of `planner/index`, and the index of each grid at
/// `indexes/<step>/index.json`. Their digest is the id of the planner release.
pub fn named<'a>(
    outputs: impl IntoIterator<Item = (&'a str, &'a [LayerFile])>,
) -> Result<Vec<(String, LayerFile)>, String> {
    let mut named = Vec::new();
    for (step, files) in outputs {
        if step == INDEX {
            let public = files.iter().filter(|file| file.path == "release.json" || file.path.starts_with("public/"));
            named.extend(public.map(|file| (file.path.clone(), file.clone())));
        } else if step.starts_with("planner/") && step.ends_with("/grid") {
            let file =
                files.iter().find(|file| file.path == "index.json").ok_or(format!("{step} has no index.json"))?;
            named.push((format!("indexes/{step}/index.json"), file.clone()));
        }
    }
    if !named.iter().any(|(path, _)| path == "release.json") {
        return Err(format!("{INDEX} has no release.json"));
    }
    named.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(named)
}

/// Write `catalog.json`: the URLs of the planner release whose id is the digest of the named files.
pub fn pointer(request: &Request) -> Result<(), String> {
    let named = named(request.layer_files.iter().map(|(step, files)| (step.as_str(), files.as_slice())))?;
    let id = obc_data::engine::digest(named.iter().map(|(path, file)| (path.as_str(), file.sha256.as_str())));
    let origins: Origins = serde_json::from_value(request.options["origins"].clone()).map_err(|e| e.to_string())?;
    let path = request.layers.get(INDEX).and_then(|files| files.get("release.json")).ok_or("no release.json")?;
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let document: Value = serde_json::from_slice(&bytes).map_err(|e| format!("release.json: {e}"))?;
    let tiles = format!("{}/releases/{id}", origins.tiles_origin);
    let files = document["files"].as_object().ok_or("planner index has no files")?;
    let layers: BTreeMap<_, _> = ["snow", "climate", "sun"]
        .into_iter()
        .filter(|kind| files.contains_key(&format!("maps/{kind}.json")))
        .map(|kind| (kind, format!("{tiles}/{kind}.json")))
        .collect();
    let active = json!({
        "device_catalog": format!("{}/cell-catalog/catalog.json", origins.objects_origin),
        "routing": format!("{}/planner-api/releases/{id}/routing", origins.api_origin),
        "search": format!("{}/planner-api/releases/{id}/search", origins.api_origin),
        "downloads": format!("{}/planner-api/releases/{id}/downloads", origins.api_origin),
        "id": id, "manifest": format!("{}/releases/{id}/release.json", origins.objects_origin),
        "region": document["region"], "name": document["name"], "bounds": document["bounds"],
        "basemap": format!("{tiles}/basemap.json"), "places": format!("{tiles}/places.json"),
        "overlays": format!("{tiles}/overlays.json"), "terrain": format!("{tiles}/terrain/{{z}}/{{x}}/{{y}}.webp"),
        "glyphs": format!("{tiles}/maps/assets/fonts/{{fontstack}}/{{range}}.pbf"),
        "sprites": format!("{tiles}/maps/assets/sprites/v4"), "routes": format!("{tiles}/routes/tiles/{{cell}}.json"),
        "attribution": document["attribution"], "landcover_attribution": document["landcover_attribution"],
        "terrain_attribution": document["terrain_attribution"], "layers": layers,
    });
    let pointer = json!({"format": 1, "origins": origins, "active": active});
    let body = serde_json::to_vec(&pointer).map_err(|e| e.to_string())?;
    std::fs::write(request.output.join("catalog.json"), body).map_err(|e| e.to_string())
}

fn checked(store: &Store, file: &LayerFile) -> Result<std::path::PathBuf, String> {
    let path = store.ensure(&file.sha256)?;
    if hash_file(&path)? != (file.sha256.clone(), file.size) {
        return Err(format!("planner artifact `{}` differs from its receipt", file.path));
    }
    Ok(path)
}

fn descriptor(release: &Release, store: &Store) -> Result<Vec<u8>, String> {
    let index = release.outputs.iter().find(|output| output.step == INDEX).ok_or("release has no planner index")?;
    let file = index.files.iter().find(|file| file.path == "release.json").ok_or("planner has no release.json")?;
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
    let body = descriptor(release, store)?;
    let document: Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    if document["region"] != release.settings.region.rsplit('/').next().unwrap_or("")
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
    for (path, file) in named(release.outputs.iter().map(|output| (output.step.as_str(), output.files.as_slice())))? {
        paths.insert(path, checked(store, &file)?);
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
        planner_router::open(&source.join("runtime/routing"))
            .and_then(|graph| graph.verify())
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
