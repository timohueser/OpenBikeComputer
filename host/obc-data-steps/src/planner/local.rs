//! Matching-host execution is separate from portable published planner data.

mod data;

use std::path::Path;

use obc_data::engine::{Code, Profile, Rust, Step};
use serde_json::json;

fn native(root: &Path, package: &str) -> Result<Step, String> {
    let mut step = crate::python(
        &format!("local/{package}"),
        Vec::new(),
        json!({}),
        ("tools.planner_local_route", None),
        &["tools/planner_local_route.py"],
        &[package],
    );
    step.code.crates = vec![package.into()];
    step.code.rust = Some(Rust::Native { profile: Profile::Release });
    let files = step.code.files(root)?;
    step.options = json!({"package": package, "root": root.canonicalize().map_err(|e| e.to_string())?, "code": obc_data::engine::digest(files.iter().map(|(key, value)| (key.as_str(), value.as_str())))});
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
        python: Some(obc_data::engine::Python { group: None }),
        ..Default::default()
    }
}

use obc_data::dev::{Binding, Prepared, Request};
use obc_data::engine::release;
use obc_data::engine::runs::{self, Event, Phase};
use obc_data::env::Env;
use obc_data::fetch::http::Http;
use obc_data::live::{Live, Remote};
use obc_data::product::Unplanned;
use obc_data::regions::Regions;
use obc_data::sources::Registry;
use obc_data::store::{hash_file, sha256_hex, Store};
use std::collections::BTreeMap;

pub(super) fn environment(root: &Path, regions: &Regions, request: &Request) -> Result<Env, String> {
    let (env, text) = read_environment(root, regions, request)?;
    obc_data::store::write_atomic(&Env::path(root, "local"), text.as_bytes())?;
    Ok(env)
}

fn read_environment(root: &Path, regions: &Regions, request: &Request) -> Result<(Env, String), String> {
    let (env, text) = Env::local(root, regions, request.region.as_deref())?;
    let text = env.edit(&text);
    Ok((env, text))
}

fn kinds(app: obc_data::dev::App) -> &'static [data::Kind] {
    match app {
        obc_data::dev::App::WebPlanner => &[data::Kind::Planner],
        _ => &[data::Kind::Maps],
    }
}

fn selected_apps(store: &Store, request: &Request) -> Result<std::collections::BTreeSet<obc_data::dev::App>, String> {
    let mut apps: std::collections::BTreeSet<_> = obc_data::dev::state(store)?
        .into_iter()
        .flat_map(|state| state.apps)
        .filter(|(_, state)| state.status == "ready")
        .map(|(app, _)| app)
        .collect();
    apps.insert(request.app);
    Ok(apps)
}

fn selected_kinds(apps: &std::collections::BTreeSet<obc_data::dev::App>) -> Vec<data::Kind> {
    let mut selected = Vec::new();
    for app in apps {
        for kind in kinds(*app) {
            if !selected.contains(kind) {
                selected.push(*kind);
            }
        }
    }
    selected
}

pub(super) fn check(root: &Path, store: &Store, request: &Request) -> Result<obc_data::cli::EnvPlan, String> {
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let regions = obc_data::settings::regions(&root, store)?;
    let registry = Registry::effective(&root, store)?;
    let (env, _) = read_environment(&root, &regions, request)?;
    let remote = request.refresh_live.then(Remote::from_env).transpose()?;
    let mut checked = obc_data::cli::EnvPlan {
        env: "local".into(),
        region: env.region.clone(),
        layers: env.layers.clone(),
        ..Default::default()
    };
    let apps = selected_apps(store, request)?;
    for kind in selected_kinds(&apps) {
        let product = kind.product();
        let result = (|| -> Result<(), String> {
            if request.reviewed.is_none()
                && !request.refresh_live
                && !obc_data::local::saved(store)?.iter().any(|saved| saved.original.product == product.name())
            {
                return Err("Prepare Local inputs to select the initial Live versions".into());
            }
            let (selected, live, mut steps) =
                data::Inputs { root: &root, store, regions: &regions, registry: &registry }.metadata(
                    env.clone(),
                    kind,
                    request.refresh_live,
                    request.reviewed.as_deref(),
                    remote.as_ref(),
                    None,
                )?;
            let prior = live.releases().next().map(|(_, _, release)| release);
            let reused = prior
                .map(|prior| obc_data::local::reusable(&root, store, product, prior, &steps))
                .transpose()?
                .unwrap_or_default();
            if steps
                .iter()
                .any(|step| !reused.contains_key(&step.name) && step.code.crates.iter().any(|name| name == "obc-osm"))
            {
                let declared = kind
                    .declarations(
                        &root,
                        &selected,
                        &regions,
                        store,
                        obc_osm::OsmiumRunner::default().binding().map(Some),
                    )
                    .map_err(|e| format!("{e:?}"))?;
                if !declared.blocked.is_empty() {
                    return Err(format!("Local input execution is blocked: {:?}", declared.blocked));
                }
                steps = declared.steps;
            }
            checked.groups.extend(obc_data::engine::plan::plan_reusing(store, &root, &steps, &reused)?.groups);
            checked.versions.extend(selected.read.borrow().iter().map(|((source, params), version)| {
                obc_data::cli::FetchVersion {
                    product: Some(kind.product().name().into()),
                    source: source.clone(),
                    params: params.clone(),
                    version: version.clone(),
                }
            }));
            checked.live.push(obc_data::cli::LiveRelease {
                product: product.name().into(),
                release: prior.map(|release| release.id()),
                pointer: None,
                observed: None,
                key: format!("{}/catalog.json", product.prefix()),
            });
            Ok(())
        })();
        if let Err(reason) = result {
            checked.needs_prepare |= reason.starts_with("Prepare Local");
            checked.blocked.push(obc_data::cli::BlockedProduct {
                product: product.name().into(),
                reason,
                layers: Vec::new(),
            });
        }
    }
    if checked.blocked.is_empty() {
        for (app, package) in
            [(obc_data::dev::App::WebPlanner, "planner-service"), (obc_data::dev::App::Simulator, "obc-sim")]
        {
            if !apps.contains(&app) {
                continue;
            }
            match native(&root, package).and_then(|step| obc_data::engine::plan::plan(store, &root, &[step])) {
                Ok(plan) => checked.groups.extend(plan.groups),
                Err(reason) => checked.blocked.push(obc_data::cli::BlockedProduct {
                    product: app.name().into(),
                    reason,
                    layers: Vec::new(),
                }),
            }
        }
    }
    if checked.blocked.is_empty() {
        let browser = apps.iter().any(|app| *app != obc_data::dev::App::Simulator);
        let providers = services().files(&root).and_then(|_| {
            if browser {
                providers(&root, apps.contains(&obc_data::dev::App::WebPlanner)).map(drop)
            } else {
                Ok(())
            }
        });
        if let Err(reason) = providers {
            checked.blocked.push(obc_data::cli::BlockedProduct {
                product: request.app.name().into(),
                reason,
                layers: Vec::new(),
            });
        }
    }
    Ok(checked)
}

pub(super) fn inputs(
    root: &Path,
    store: &Store,
    request: &Request,
    run: &mut runs::Run,
) -> Result<obc_data::cli::EnvPlan, String> {
    let regions = obc_data::settings::regions(root, store)?;
    let registry = Registry::effective(root, store)?;
    let env = environment(root, &regions, request)?;
    let remote = Remote::from_env()?;
    let mut pinned = obc_data::cli::EnvPlan {
        env: "local".into(),
        region: env.region.clone(),
        layers: env.layers.clone(),
        ..Default::default()
    };
    let apps = selected_apps(store, request)?;
    for kind in selected_kinds(&apps) {
        let (selected, live, _) = data::Inputs { root, store, regions: &regions, registry: &registry }.metadata(
            env.clone(),
            kind,
            request.refresh_live,
            None,
            Some(&remote),
            Some(run),
        )?;
        pinned.versions.extend(selected.read.borrow().iter().map(|((source, params), version)| {
            obc_data::cli::FetchVersion {
                product: Some(kind.product().name().into()),
                source: source.clone(),
                params: params.clone(),
                version: version.clone(),
            }
        }));
        pinned.live.extend(live.products.iter().map(|product| obc_data::cli::LiveRelease {
            product: product.product.clone(),
            release: product.release.as_ref().map(|(id, _)| id.clone()),
            pointer: None,
            observed: product.observed.clone(),
            key: format!("{}/catalog.json", product.prefix),
        }));
    }
    let mut request = request.clone();
    request.reviewed = Some(Box::new(pinned));
    check(root, store, &request)
}

fn view_identity(
    configuration: &str,
    releases: &BTreeMap<String, release::Release>,
    supervisor: &Binding,
    children: &BTreeMap<String, Binding>,
    executables: &BTreeMap<String, obc_data::engine::LayerFile>,
) -> Result<String, String> {
    Ok(sha256_hex(
        &serde_json::to_vec(&(
            releases.iter().map(|(name, release)| (name, release.id())).collect::<BTreeMap<_, _>>(),
            configuration,
            supervisor,
            children.iter().map(|(name, binding)| (name, &binding.files)).collect::<BTreeMap<_, _>>(),
            executables,
        ))
        .map_err(|e| e.to_string())?,
    ))
}

pub(super) fn prepare(root: &Path, store: &Store, request: &Request, run: &mut runs::Run) -> Result<Prepared, String> {
    use obc_data::dev::App;
    if let Some(reviewed) = &request.reviewed {
        let current = check(root, store, request)?;
        if current != **reviewed {
            return Err("Local plan changed; review the current pending work again".into());
        }
    }
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let regions = obc_data::settings::regions(&root, store)?;
    let registry = Registry::effective(&root, store)?;
    let env = environment(&root, &regions, request)?;
    let configuration = obc_data::dev::configuration_for(&env, &regions)?;
    let apps = selected_apps(store, request)?;
    let browser = apps.iter().any(|app| *app != App::Simulator);
    let web = apps.contains(&App::WebPlanner);
    let maps = apps.iter().any(|app| *app != App::WebPlanner);
    let mut releases = BTreeMap::new();
    for (needed, kind) in [(web, data::Kind::Planner), (maps, data::Kind::Maps)] {
        if needed {
            let release = data::Inputs { root: &root, store, regions: &regions, registry: &registry }.build(
                env.clone(),
                kind,
                request.refresh_live,
                request.reviewed.as_deref(),
                run,
            )?;
            releases.insert(release.product.clone(), release);
        }
    }
    let mut executables = BTreeMap::new();
    let mut children = BTreeMap::new();
    run.reuse_layers(&BTreeMap::new());
    for (needed, child, package) in
        [(web, "routing", "planner-service"), (apps.contains(&App::Simulator), "simulator", "obc-sim")]
    {
        if needed {
            let step = native(&root, package)?;
            let work = obc_data::engine::plan::plan(store, &root, std::slice::from_ref(&step))?;
            run.build(
                &runs::Context {
                    store,
                    root: &root,
                    sources: &registry.sources,
                    http: &Http::new(),
                    copies: None,
                    limits: runs::Limits::machine(),
                },
                std::slice::from_ref(&step),
                &work,
            )?;
            let artifacts = release::stored(
                store,
                &root,
                std::slice::from_ref(&step),
                &std::collections::BTreeSet::from([step.name.as_str()]),
            )?;
            let file = artifacts
                .values()
                .next()
                .ok_or("Local native build has no artifact")?
                .files
                .iter()
                .find(|file| file.path == package)
                .ok_or("Local native build lacks its executable")?
                .clone();
            let mut code = step.code;
            code.libraries.push(obc_data::engine::Library {
                version: None,
                name: format!("local-{child}"),
                path: store.object(&file.sha256).canonicalize().map_err(|e| e.to_string())?,
                sha256: file.sha256.clone(),
            });
            children.insert(child.into(), Binding { files: code.files(&root)?, code });
            executables.insert(package.to_string(), file);
        }
    }
    let code = services();
    let supervisor = Binding { files: code.files(&root)?, code };
    let node = if browser {
        let (providers, node) = providers(&root, web)?;
        children.extend(providers);
        Some(node)
    } else {
        None
    };
    let identity = view_identity(&configuration, &releases, &supervisor, &children, &executables)?;
    let directory = store.root().join("dev/local");
    let view = directory.join("views").join(identity);
    for (child, package) in [("routing", "planner-service"), ("simulator", "obc-sim")] {
        if let Some(binding) = children.get_mut(child) {
            binding
                .code
                .libraries
                .iter_mut()
                .find(|library| library.name == format!("local-{child}"))
                .ok_or("Native Local binding disappeared")?
                .path = view.join(package);
        }
    }
    let mut service = json!({"root":root, "view":view, "node":node, "region":env.region, "layers":env.layers, "configuration":configuration, "executables":executables.iter().map(|(name,file)| (name.clone(),file.sha256.clone())).collect::<BTreeMap<_,_>>()});
    if let Some(original) = releases.get("planner") {
        let file = original.named.iter().find(|file| file.path == "release.json").ok_or("planner has no manifest")?;
        if hash_file(&store.object(&file.sha256))? != (file.sha256.clone(), file.size) {
            return Err("Local planner manifest changed".into());
        }
        let document: serde_json::Value =
            serde_json::from_slice(&std::fs::read(store.object(&file.sha256)).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        service["release"] = json!(original.id());
        service["expected"] = json!({"routing":document["routing_package"], "search":document["files"][format!("search/{}.grid.json", env.region)]["sha256"], "model": (["labels.json", "tokenizer.json", "model.int8.onnx"].into_iter().map(|name| (name,document["files"][format!("search/model/{name}")]["sha256"].clone())).collect::<BTreeMap<_,_>>())});
    }
    if let Some(original) = releases.get("maps") {
        service["maps_release"] = json!(original.id());
    }
    let fingerprints: BTreeMap<_, _> = children
        .iter()
        .map(|(name, binding)| {
            let data = match name.as_str() {
                "routing" => service["expected"]["routing"].clone(),
                "search" => service["expected"].clone(),
                "simulator" => service["maps_release"].clone(),
                "tiles" | "frontend" => json!([service["release"], service["maps_release"]]),
                _ => unreachable!("known child"),
            };
            (name, json!([binding.files, data]))
        })
        .collect();
    service["fingerprints"] = json!(fingerprints);
    let simulator_map = if apps.contains(&App::Simulator) {
        let scratch =
            tempfile::Builder::new().prefix("obc-local-map-").tempdir_in(store.root()).map_err(|e| e.to_string())?;
        crate::maps::catalog::assemble(
            releases.get("maps").ok_or("Simulator has no Maps release")?,
            store,
            &scratch.path().join("map.obcm"),
        )?;
        service["map_sha256"] = json!(hash_file(&scratch.path().join("map.obcm"))?.0);
        Some(scratch)
    } else {
        None
    };
    let descriptor = serde_json::to_vec(&service).map_err(|e| e.to_string())?;
    if view.exists() {
        if std::fs::read(view.join("service.json")).map_err(|e| e.to_string())? != descriptor {
            return Err("Prepared Local view differs from its current declarations".into());
        }
    } else {
        let partial = directory.join("views").join(format!(".{}", run.id()));
        std::fs::create_dir_all(&partial).map_err(|e| e.to_string())?;
        for (name, original) in &releases {
            let prefix = if name == "maps" { "cell-catalog" } else { "planner" };
            link_release(store, original, &partial.join(prefix))?;
        }
        for (package, file) in &executables {
            let source = store.object(&file.sha256);
            if hash_file(&source)? != (file.sha256.clone(), file.size) {
                return Err("Local native artifact changed".into());
            }
            let binary = partial.join(package);
            std::fs::copy(source, &binary).map_err(|e| e.to_string())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o500)).map_err(|e| e.to_string())?;
            }
        }
        if web {
            let mut command = supervisor.code.command(
                &root,
                &[
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
                    "--prepare",
                ]
                .map(String::from),
            )?;
            if !command.arg(&partial).status().map_err(|e| e.to_string())?.success() {
                return Err("Local data materialization failed".into());
            }
        }
        if let Some(map) = &simulator_map {
            std::fs::copy(map.path().join("map.obcm"), partial.join("map.obcm")).map_err(|e| e.to_string())?;
        }
        obc_data::store::durable(&partial.join("service.json"), &descriptor)?;
        std::fs::rename(&partial, &view).map_err(|e| e.to_string())?;
        obc_data::store::durable_directory(view.parent().expect("view directory"))?;
    }
    let prepared = Prepared { descriptor: hash_file(&view.join("service.json"))?.0, view, supervisor, children, apps };
    prepared.check(&root)?;
    Ok(prepared)
}

fn link_release(store: &Store, release: &release::Release, directory: &Path) -> Result<(), String> {
    for (kind, file) in release.publication().files() {
        let relative = match kind {
            release::Published::Manifest => continue,
            release::Published::Named("release.json") if release.product == "planner" => "release.json",
            _ => &file.path,
        };
        link(store, &file, &directory.join(relative))?;
    }
    Ok(())
}

fn link(store: &Store, file: &obc_data::engine::LayerFile, destination: &Path) -> Result<(), String> {
    let original = store.object(&file.sha256);
    if hash_file(&original)? != (file.sha256.clone(), file.size) {
        return Err("Local view object differs from its recorded bytes".into());
    }
    if destination.exists() {
        return if hash_file(destination)? == (file.sha256.clone(), file.size) {
            Ok(())
        } else {
            Err("Local view path has different recorded bytes".into())
        };
    }
    std::fs::create_dir_all(destination.parent().expect("view path")).map_err(|e| e.to_string())?;
    std::fs::hard_link(original, destination).map_err(|e| e.to_string())
}

fn providers(root: &Path, web: bool) -> Result<(BTreeMap<String, Binding>, std::path::PathBuf), String> {
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
    let executable = obc_data::engine::Library {
        version: Some(String::from_utf8(version.stdout).map_err(|e| e.to_string())?),
        name: "local-node".into(),
        path: node.clone(),
        sha256: hash_file(&node)?.0,
    };
    let mut children = BTreeMap::new();
    for (name, app) in [("search", "planner/search"), ("tiles", "planner/tiles"), ("frontend", "builder/web")] {
        if name == "search" && !web {
            continue;
        }

        let mut code = Code {
            paths: vec![app.into()],
            libraries: vec![executable.clone()],
            python: (name == "search").then(|| obc_data::engine::Python { group: Some("search-runtime".into()) }),
            ..Default::default()
        };
        if name == "frontend" {
            for (role, file) in [("bridge", "obc_builder_bridge.js"), ("wasm", "obc_builder_bridge_bg.wasm")] {
                let path = root
                    .join("builder/web/src/lib/core/pkg")
                    .join(file)
                    .canonicalize()
                    .map_err(|e| format!("Prepare the builder bridge/Wasm with the builder README first: {e}"))?;
                code.libraries.push(obc_data::engine::Library {
                    version: None,
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
                        version: None,
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
                        version: None,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_view_uses_published_names_for_distinct_grid_indexes() {
        let scratch = tempfile::tempdir().unwrap();
        let store = Store::at(scratch.path().join("store"));
        let mut release = release::Release::compose("planner", "ride", &[], None, Vec::new(), &Default::default());
        let stored = |path: &str, bytes: &[u8]| {
            let source = scratch.path().join("source");
            std::fs::write(&source, bytes).unwrap();
            let (sha256, size) = hash_file(&source).unwrap();
            store.insert(&source, &sha256).unwrap();
            obc_data::engine::LayerFile { path: path.into(), sha256, size }
        };
        let object = stored(&format!("objects/{}", sha256_hex(b"tile")), b"tile");
        let private = stored("private", b"intermediate");
        for name in ["indexes/places/grid/index.json", "indexes/basemap/grid/index.json", "release.json"] {
            let file = stored("index.json", name.as_bytes());
            release.layers.push(release::Layer {
                step: name.into(),
                key: String::new(),
                inputs: Vec::new(),
                options: json!({}),
                code: String::new(),
                command: None,
                outputs: vec!["index.json".into(), "objects".into(), "private".into()],
                digest: String::new(),
                files: vec![file.clone(), object.clone(), private.clone()],
                snapshots: BTreeMap::new(),
                client: obc_data::engine::Client::Paths(vec!["objects".into()]),
            });
            release.named.push(obc_data::engine::LayerFile { path: name.into(), ..file });
        }
        release.name_files(release.named.clone()).unwrap();
        let view = scratch.path().join("planner");
        link_release(&store, &release, &view).unwrap();
        for file in &release.named {
            let path = if file.path == "release.json" {
                view.join(&file.path)
            } else {
                view.join("releases").join(release.id()).join(&file.path)
            };
            assert_eq!(std::fs::read_to_string(path).unwrap(), file.path);
        }
        assert_eq!(std::fs::read(view.join(&object.path)).unwrap(), b"tile");
        assert_eq!(std::fs::read_dir(view.join("objects")).unwrap().count(), 1);
        assert!(!view.join(format!("objects/{}", private.sha256)).exists());
        assert!(!view.join("private").exists());

        let id = release.id();
        let live = Live {
            products: vec![obc_data::live::LiveProduct {
                product: "planner".into(),
                prefix: "planner".into(),
                release: Some((id.clone(), release)),
                applied: None,
                commit: None,
                document: None,
                observed: None,
            }],
            inputs: BTreeMap::new(),
        };
        let expected = live.expected();
        assert_eq!(expected.len(), 6, "pointer, manifest, three aliases and one shared object");
        for (key, size) in expected {
            let relative = key.strip_prefix("planner/").unwrap();
            let local = match relative {
                "catalog.json" => continue,
                path if path == format!("releases/{id}.json") => continue,
                path if path == format!("releases/{id}/release.json") => "release.json",
                path => path,
            };
            assert_eq!(std::fs::metadata(view.join(local)).unwrap().len(), size.unwrap());
        }
    }

    #[test]
    fn view_layers_share_identical_objects_but_reject_path_conflicts() {
        let scratch = tempfile::tempdir().unwrap();
        let store = Store::at(scratch.path().join("store"));
        let source = scratch.path().join("source");
        let destination = scratch.path().join("view/object");
        for bytes in [b"one", b"two"] {
            std::fs::write(&source, bytes).unwrap();
            let (sha256, size) = hash_file(&source).unwrap();
            store.insert(&source, &sha256).unwrap();
            let file = obc_data::engine::LayerFile { path: "object".into(), sha256, size };
            if bytes == b"one" {
                link(&store, &file, &destination).unwrap();
                link(&store, &file, &destination).unwrap();
            } else {
                assert!(link(&store, &file, &destination).unwrap_err().contains("different recorded bytes"));
            }
        }
        assert_eq!(std::fs::read(destination).unwrap(), b"one");
    }

    #[test]
    fn equivalent_product_bytes_get_a_new_view_for_changed_captured_configuration() {
        let regions =
            Regions::new(vec![
                obc_data::regions::parse_region("ride", "name='Ride'\nkind='box'\nbox=[7,47,8,48]\n").unwrap()
            ])
            .unwrap();
        let mut env = Env { region: "ride".into(), ..Default::default() };
        let original = obc_data::dev::configuration_for(&env, &regions).unwrap();
        let supervisor = Binding { code: Code::default(), files: Default::default() };
        let identity = |configuration: &str| {
            view_identity(configuration, &BTreeMap::new(), &supervisor, &BTreeMap::new(), &BTreeMap::new()).unwrap()
        };
        let prior = identity(&original);
        env.layers.push("sun".into());
        let changed = obc_data::dev::configuration_for(&env, &regions).unwrap();
        assert_ne!(
            prior,
            identity(&changed),
            "an unused Planner option still changes the prepared Local configuration"
        );
        assert_eq!(prior, identity(&original), "the preparation snapshot remains stable despite later edits");
    }
}
