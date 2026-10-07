//! Matching-host execution is separate from portable published planner data.

use std::path::Path;

use obc_data::engine::{Code, Profile, Rust, Step};
use serde_json::json;

pub(super) fn routing(root: &Path) -> Result<Step, String> {
    let mut step = crate::python(
        "local/route-service",
        Vec::new(),
        json!({}),
        ("tools.planner_local_route", None),
        &["tools/planner_local_route.py"],
        &["route-server"],
    );
    step.code.crates = vec!["route-server".into()];
    step.code.rust = Some(Rust::Native { profile: Profile::Release });
    let files = step.code.files(root)?;
    step.options = json!({"root": root.canonicalize().map_err(|e| e.to_string())?, "code": obc_data::engine::digest(files.iter().map(|(key, value)| (key.as_str(), value.as_str())))});
    Ok(step)
}

pub(super) fn services() -> Code {
    Code {
        paths: [
            "tools/planner_local.py",
            "tools/planner_maps.py",
            "tools/planner_geo.py",
            "tools/planner_offline.py",
            "tools/planner_runtime.py",
        ]
        .map(String::from)
        .into(),
        python: Some(obc_data::engine::Python { group: Some("search-runtime".into()) }),
        ..Default::default()
    }
}

use obc_data::dev::{Binding, Prepared, Request, Source};
use obc_data::engine::release;
use obc_data::engine::runs::{self, Event, Phase};
use obc_data::env::Env;
use obc_data::fetch::http::Http;
use obc_data::live::{Live, Remote};
use obc_data::product::Unplanned;
use obc_data::regions::Regions;
use obc_data::sources::Registry;
use obc_data::store::{hash_file, sha256_hex, Store};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    region: String,
    source: Source,
    release: String,
}

pub(super) fn prepare(root: &Path, store: &Store, request: &Request, run: &mut runs::Run) -> Result<Prepared, String> {
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let regions = Regions::load(&root)?;
    let registry = Registry::load(&root)?;
    let mut env = Env::load(&root, "live", &regions)?;
    let directory = store.root().join("dev/local");
    let previous: Option<Selection> = match std::fs::read(directory.join("selection.json")) {
        Ok(bytes) => Some(serde_json::from_slice(&bytes).map_err(|e| e.to_string())?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.to_string()),
    };
    env.region =
        request.region.clone().or_else(|| previous.as_ref().map(|saved| saved.region.clone())).unwrap_or(env.region);
    if regions.get(&env.region).is_none() {
        return Err("Local region is not a saved definition".into());
    }
    let source = request.source.or_else(|| previous.as_ref().map(|saved| saved.source)).unwrap_or(Source::Live);
    let saved = obc_data::local::saved(store)?.into_iter().find(|saved| saved.original.product == "planner");
    let pinned = previous
        .as_ref()
        .filter(|previous| previous.source == source)
        .map(|previous| {
            saved
                .filter(|saved| saved.original.id() == previous.release)
                .map(|saved| saved.original)
                .ok_or("saved Local selection has no matching adoption root")
        })
        .transpose()?;
    let remote = Remote::from_env()?;
    let live = if let Some(original) = pinned {
        Live {
            products: vec![obc_data::live::LiveProduct {
                product: "planner".into(),
                prefix: "planner".into(),
                release: Some((original.id(), original)),
                applied: None,
                document: None,
                observed: None,
            }],
            inputs: Default::default(),
        }
    } else {
        Live::read_products(&remote, &[("planner", "planner")], &registry.sources, store)?
    };
    if source == Source::Live && live.releases().next().is_none() {
        return Err("Live has no planner release; select Local to build it".into());
    }
    env.name = "local".into();
    env.live = live.versions();
    env.retained = obc_data::input_copy::retained(&live, store)?;
    let http = Http::new();
    let copies = obc_data::input_copy::Restore { remote: &remote, live: &live };
    let execution = source == Source::Local;
    let tool = if execution { obc_osm::OsmiumRunner::default().binding().map(Some) } else { Ok(None) };
    let steps = loop {
        run.check_stop(store)?;
        match super::Planner.declarations(&root, &env, &regions, store, tool.clone(), false) {
            Ok(steps) if steps.blocked.is_empty() => break steps.steps,
            Ok(steps) => return Err(format!("Local planner is blocked: {:?}", steps.blocked)),
            Err(Unplanned::NeedsFetch(wanted)) => {
                if wanted.is_empty() {
                    return Err("Local metadata discovery made no progress".into());
                }
                for wanted in wanted {
                    if !execution
                        && !["geofabrik-index", "geofabrik-poly", crate::maps::TILE_LIST]
                            .contains(&wanted.source.as_str())
                    {
                        return Err(format!(
                            "Live has no current {} input selection; select Local to build",
                            wanted.source
                        ));
                    }
                    let source =
                        registry.sources.iter().find(|s| s.id == wanted.source).ok_or("unknown Local source")?;
                    let fetched = run.fetch_request(
                        &root,
                        store,
                        &http,
                        Some(&copies),
                        &obc_data::fetch::Request { source, version: wanted.version, params: wanted.params.clone() },
                        &[],
                    )?;
                    env.resolved.insert((source.id.clone(), obc_data::store::sorted(&wanted.params)), fetched.version);
                }
            }
            Err(Unplanned::Invalid(reason) | Unplanned::Failed(reason)) => return Err(reason),
        }
    };
    let original = if execution {
        run.record(&Event::Phase { phase: Phase::Build })?;
        let work = obc_data::engine::plan::plan(store, &root, &steps)?;
        run.build(
            &runs::Context {
                store,
                root: &root,
                sources: &registry.sources,
                http: &http,
                copies: Some(&copies),
                limits: runs::Limits::machine(),
            },
            &steps,
            &work,
        )?;
        let mut release = release::release(store, &root, "planner", &env.region, &env.layers, &steps)?
            .ok_or("Local build has no complete planner release")?;
        release.name_files(super::catalog::named(&release)?)?;
        release.write(store)?;
        release
    } else {
        live.releases().next().expect("checked Live").2.clone()
    };
    let index = original.layers.iter().find(|layer| layer.step == "planner/index").ok_or("planner has no index")?;
    let required = BTreeMap::from([(
        "planner/index".into(),
        index
            .files
            .iter()
            .filter(|file| file.path == "release.json" || file.path.starts_with("public/"))
            .map(|file| file.path.clone())
            .collect(),
    )]);
    let selection = obc_data::local::plan(&root, store, &super::Planner, &original, &steps, &required)?;
    if !selection.blocked.is_empty() {
        return Err(format!(
            "Live data does not match this Local request: {:?}; select Local to build",
            selection.blocked
        ));
    }
    run.record(&Event::Phase { phase: Phase::Verify })?;
    obc_data::local::adopt(&root, store, &remote, &super::Planner, &original, &steps, &required, &selection)?;
    run.record(&Event::Phase { phase: Phase::Build })?;
    let route = routing(&root)?;
    let work = obc_data::engine::plan::plan(store, &root, std::slice::from_ref(&route))?;
    run.build(
        &runs::Context {
            store,
            root: &root,
            sources: &registry.sources,
            http: &http,
            copies: None,
            limits: runs::Limits::machine(),
        },
        std::slice::from_ref(&route),
        &work,
    )?;
    let artifacts = release::stored(
        store,
        &root,
        std::slice::from_ref(&route),
        &std::collections::BTreeSet::from([route.name.as_str()]),
    )?;
    let executable = artifacts
        .values()
        .next()
        .ok_or("Local route service has no artifact")?
        .files
        .iter()
        .find(|file| file.path == "route-server")
        .ok_or("Local route service lacks its executable")?;
    let code = services();
    let supervisor = Binding { files: code.files(&root)?, code };
    let (children, node) = providers(&root)?;
    let identity = sha256_hex(
        &serde_json::to_vec(&(original.id(), &supervisor, &children, &executable.sha256)).map_err(|e| e.to_string())?,
    );
    let view = directory.join("views").join(identity);
    let manifest = original.named.iter().find(|file| file.path == "release.json").ok_or("planner has no manifest")?;
    if hash_file(&store.object(&manifest.sha256))? != (manifest.sha256.clone(), manifest.size) {
        return Err("Local planner manifest changed".into());
    }
    let document: serde_json::Value =
        serde_json::from_slice(&std::fs::read(store.object(&manifest.sha256)).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let expected = json!({"routing": document["routing_package"],
            "search": document["files"][format!("search/{}.grid.json", document["region"].as_str().ok_or("missing region")?)]["sha256"],
            "model": (["labels.json", "tokenizer.json", "model.int8.onnx"].into_iter().map(|name| (
                name, document["files"][format!("search/model/{name}")]["sha256"].clone())).collect::<BTreeMap<_, _>>())});
    let service = json!({"root":root, "view":view, "node":node, "release":original.id(), "region":document["region"], "expected":expected, "routing_executable":executable.sha256,
            "fingerprints": {"routing":executable.sha256.clone()+expected["routing"].as_str().ok_or("missing routing identity")?,
                "search":(&children["search"].files, &expected["search"], &expected["model"]),
                "tiles":(&children["tiles"].files, original.id()), "frontend":(&children["frontend"].files, original.id())}});
    let descriptor = serde_json::to_vec(&service).map_err(|e| e.to_string())?;
    let descriptor_sha = sha256_hex(&descriptor);
    if !view.exists() {
        let partial = directory.join("views").join(format!(".{}", run.id()));
        std::fs::create_dir_all(partial.join("planner/objects")).map_err(|e| e.to_string())?;
        for selected in &selection.layers {
            for file in &selected.files {
                let relative = if file.path.starts_with("objects/") {
                    format!("planner/{}", file.path)
                } else if file.path == "release.json" {
                    "planner/release.json".into()
                } else {
                    format!("planner/releases/{}/{}", original.id(), file.path)
                };
                link(store, file, &partial.join(relative))?;
            }
        }
        let binary = partial.join("route-server");
        let original = store.object(&executable.sha256);
        if hash_file(&original)? != (executable.sha256.clone(), executable.size) {
            return Err("Local route artifact changed".into());
        }
        std::fs::copy(original, &binary).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o500)).map_err(|e| e.to_string())?;
        }
        let mut command = supervisor.code.command(
            &root,
            &[
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
                "--prepare",
            ]
            .map(String::from),
        )?;
        if !command.arg(&partial).status().map_err(|e| e.to_string())?.success() {
            return Err("Local data materialization failed".into());
        }
        obc_data::commit::durable(&partial.join("service.json"), &descriptor)?;
        std::fs::rename(&partial, &view).map_err(|e| e.to_string())?;
        obc_data::commit::durable_directory(view.parent().expect("view directory"))?;
    }
    let prepared = Prepared { view, descriptor: descriptor_sha, supervisor, children };
    prepared.check(&root)?;
    obc_data::commit::durable(
        &directory.join("selection.json"),
        &serde_json::to_vec(&Selection { region: env.region, source, release: original.id() })
            .map_err(|e| e.to_string())?,
    )?;
    Ok(prepared)
}

fn link(store: &Store, file: &obc_data::engine::LayerFile, destination: &Path) -> Result<(), String> {
    let original = store.object(&file.sha256);
    if hash_file(&original)? != (file.sha256.clone(), file.size) {
        return Err("Local view object differs from its recorded bytes".into());
    }
    std::fs::create_dir_all(destination.parent().expect("view path")).map_err(|e| e.to_string())?;
    std::fs::hard_link(original, destination).map_err(|e| e.to_string())
}

fn providers(root: &Path) -> Result<(BTreeMap<String, Binding>, std::path::PathBuf), String> {
    let node = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .map(|path| path.join("node"))
        .find(|path| path.is_file())
        .ok_or("Prepare Node 24 and the three app dependency trees first")?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let version = std::process::Command::new(&node).arg("--version").output().map_err(|e| e.to_string())?;
    if !version.status.success()
        || std::str::from_utf8(&version.stdout)
            .ok()
            .and_then(|version| version.trim().strip_prefix('v')?.split('.').next()?.parse::<u32>().ok())
            .is_none_or(|major| major < 24)
    {
        return Err("Local planner services need prepared Node 24 or newer".into());
    }
    let executable =
        obc_data::engine::Library { name: "local-node".into(), path: node.clone(), sha256: hash_file(&node)?.0 };
    let mut children = BTreeMap::new();
    for (name, app) in [("search", "apps/planner-search"), ("tiles", "apps/planner-tiles"), ("frontend", "builder/app")]
    {
        let mut code = Code {
            paths: vec![app.into()],
            libraries: vec![executable.clone()],
            python: (name == "search").then(|| obc_data::engine::Python { group: Some("search-runtime".into()) }),
            ..Default::default()
        };
        if name == "frontend" {
            for (role, file) in [("bridge", "obc_builder_bridge.js"), ("wasm", "obc_builder_bridge_bg.wasm")] {
                let path = root
                    .join("builder/app/src/lib/core/pkg")
                    .join(file)
                    .canonicalize()
                    .map_err(|e| format!("Prepare the builder bridge/Wasm with the builder README first: {e}"))?;
                code.libraries.push(obc_data::engine::Library {
                    name: format!("local-frontend-{role}"),
                    sha256: hash_file(&path)?.0,
                    path,
                });
            }
        }
        let modules = root
            .join(app)
            .join("node_modules")
            .canonicalize()
            .map_err(|e| format!("Prepare {app} dependencies first: {e}"))?;
        let mut pending = vec![modules.clone()];
        while let Some(path) = pending.pop() {
            for entry in std::fs::read_dir(&path).map_err(|e| format!("Prepare {app} dependencies first: {e}"))? {
                let entry = entry.map_err(|e| e.to_string())?;
                let path = entry.path();
                let kind = entry.file_type().map_err(|e| e.to_string())?;
                if kind.is_symlink() {
                    let resolved = path.canonicalize().map_err(|e| e.to_string())?;
                    if resolved.is_dir() || !resolved.starts_with(&modules) {
                        return Err("Local npm dependencies must be self-contained regular files".into());
                    }
                    code.libraries.push(obc_data::engine::Library {
                        name: format!(
                            "local-{name}-{}",
                            sha256_hex(path.strip_prefix(&modules).unwrap().as_os_str().as_encoded_bytes())
                        ),
                        sha256: hash_file(&resolved)?.0,
                        path: resolved,
                    });
                } else if kind.is_dir() {
                    pending.push(path);
                } else if kind.is_file() {
                    code.libraries.push(obc_data::engine::Library {
                        name: format!(
                            "local-{name}-{}",
                            sha256_hex(path.strip_prefix(&modules).unwrap().as_os_str().as_encoded_bytes())
                        ),
                        sha256: hash_file(&path)?.0,
                        path,
                    });
                }
            }
        }
        code.libraries.sort_by(|a, b| a.name.cmp(&b.name));
        children.insert(name.into(), Binding { files: code.files(root)?, code });
    }
    Ok((children, node))
}
