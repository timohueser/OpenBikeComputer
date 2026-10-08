//! Recipe scope and reuse for raw OSM preparation.

use super::*;

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn recipe_root(target: &Path, steps: &[Step]) {
    let repository = root().canonicalize().unwrap();
    let output = std::process::Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .args(["metadata", "--offline", "--locked", "--no-deps", "--format-version", "1"])
        .current_dir(&repository)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    std::fs::create_dir_all(target).unwrap();
    for path in ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "data/sources.toml"] {
        let to = target.join(path);
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(repository.join(path), to).unwrap();
    }
    for package in metadata["packages"].as_array().unwrap() {
        let manifest = Path::new(package["manifest_path"].as_str().unwrap());
        let to = target.join(manifest.strip_prefix(&repository).unwrap());
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(manifest, to).unwrap();
        for entry in package["targets"].as_array().unwrap() {
            let source = Path::new(entry["src_path"].as_str().unwrap());
            let to = target.join(source.strip_prefix(&repository).unwrap());
            std::fs::create_dir_all(to.parent().unwrap()).unwrap();
            std::fs::write(to, "").unwrap();
        }
    }
    // Includes outside the crates are selected by the real Code resolver, not a second file inventory.
    let mut codes = Vec::new();
    for step in steps {
        if !codes.contains(&step.code) {
            codes.push(step.code.clone());
            for path in step.code.files(&repository).unwrap().keys() {
                let from = repository.join(path);
                if from.is_file() {
                    let to = target.join(path);
                    std::fs::create_dir_all(to.parent().unwrap()).unwrap();
                    std::fs::copy(from, to).unwrap();
                }
            }
        }
    }
    assert!(std::process::Command::new("git").args(["init", "-q"]).current_dir(target).status().unwrap().success());
}

fn identities(root: &Path, steps: &[Step]) -> std::collections::HashMap<Code, BTreeMap<String, String>> {
    let mut identities = std::collections::HashMap::new();
    for step in steps {
        if !identities.contains_key(&step.code) {
            identities.insert(step.code.clone(), step.code.files(root).unwrap());
        }
    }
    identities
}

fn authored_osm(request: &Request) -> Result<(), String> {
    let input = request.layers.values().flat_map(|files| files.values()).next().ok_or("no extract")?;
    std::fs::create_dir(request.output.join("osm")).map_err(|e| e.to_string())?;
    for leaf in request.options["leaves"].as_array().ok_or("no leaves")? {
        let leaf = LeafId { i: leaf[0].as_i64().ok_or("latitude")?, j: leaf[1].as_i64().ok_or("longitude")? };
        std::fs::copy(input, request.output.join(obc_osm::step::leaf_pbf(leaf))).map_err(|e| e.to_string())?;
    }
    Ok(())
}

// Stable authored bytes isolate recipe invalidation from the producer byte fixtures in obc-bake.
fn authored_map(request: &Request) -> Result<(), String> {
    if request.step == "maps/osm" {
        return authored_osm(request);
    }
    let directories: &[&str] = if request.step.starts_with("maps/terrain/") {
        &["terrain", "metadata"]
    } else if request.options.get("band").is_some() {
        &["cells", "metadata"]
    } else if request.step.starts_with("maps/landmark") {
        &["landmarks", "shared-content"]
    } else if request.step.starts_with("maps/peak") {
        &["peaks", "shared-content"]
    } else if request.step == catalog::LAYER {
        for file in ["catalog.json", "schema.json", "terrain.json", "LICENSE.txt"] {
            std::fs::write(request.output.join(file), "catalog").map_err(|e| e.to_string())?;
        }
        &["regions", "objects"]
    } else {
        return Err(format!("unexpected authored step {}", request.step));
    };
    for directory in directories {
        let path = request.output.join(directory);
        std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
        std::fs::write(path.join("fixture"), "stable bytes").map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn map_recipes(store: &Store) -> Vec<Step> {
    let (env, regions) = freiburg(store);
    with_osm(store);
    with_captures(store, "1");
    without_models(store, &env, &regions);
    Maps.steps_with_tool(&root(), &env, &regions, store, Ok(authored_tool(store.root()))).unwrap().steps
}

#[test]
fn navigation_edits_reuse_actual_drawing_source_osm_captures_and_content_recipes() {
    let temporary = temp("source-code-scope");
    let store = Store::at(temporary.0.join("store"));
    let mut steps = map_recipes(&store);
    glo30_fetched(&store, &steps, "1", |tile| tile.to_string());
    let network = steps.iter().find(|step| step.code.crates == ["obc-network"]).unwrap();
    assert!(network.inputs.iter().all(|input| !matches!(input, Input::Snapshot { source, .. } if source == LAND)));
    let root = temporary.0.join("checkout");
    recipe_root(&root, &steps);
    let before = identities(&root, &steps);
    for step in &steps {
        let files = &before[&step.code];
        if step.code.crates == ["obc-network"] {
            assert!(files.keys().any(|path| path.ends_with("obc-network/src/nav.rs")));
            assert!(files.keys().all(|path| !path.contains("obc-draw") && !path.contains("obc-bake")));
        } else {
            assert!(files.keys().all(|path| !path.contains("obc-network")), "{}", step.name);
        }
    }
    steps
        .iter_mut()
        .filter(|step| !step.name.starts_with("maps/source/"))
        .for_each(|step| step.run = Run::Rust(authored_map));
    let http = Http::new();
    let context =
        Context { root: &root, store: &store, sources: &[], http: &http, copies: None, limits: Limits::machine() };
    let initial = plan(&store, &root, &steps).unwrap();
    assert_eq!(builds(&initial).len(), steps.len());
    let mut run = RunLog::create(&store, "build scoped map recipes").unwrap();
    run.build(&context, &steps, &initial).unwrap();
    run.finish(None).unwrap();
    assert!(plan(&store, &root, &steps).unwrap().builds().next().is_none());
    let path = root.join("host/obc-network/src/nav.rs");
    let mut code = std::fs::read_to_string(&path).unwrap();
    code.push_str("\nconst RECIPE_SCOPE_PROBE: u32 = 1;\n");
    std::fs::write(path, code).unwrap();
    let changed = plan(&store, &root, &steps).unwrap();
    let network_names: Vec<_> =
        steps.iter().filter(|step| step.code.crates == ["obc-network"]).map(|step| step.name.as_str()).collect();
    assert_eq!(
        builds(&changed).into_iter().filter(|name| *name != catalog::LAYER).collect::<Vec<_>>(),
        network_names,
        "graph edits reuse source, OSM, drawing and content; catalog reads network output",
    );
    let mut run = RunLog::create(&store, "build graph change").unwrap();
    run.build(&context, &steps, &changed).unwrap();
    run.finish(None).unwrap();
    assert!(plan(&store, &root, &steps).unwrap().builds().next().is_none());
    let after = identities(&root, &steps);
    for step in &steps {
        assert_eq!(after[&step.code] == before[&step.code], step.code.crates != ["obc-network"], "{}", step.name);
    }
}

#[test]
fn catalog_credit_identity_is_scoped_to_catalog_not_cells_or_osm() {
    let temporary = temp("catalog-credit-code");
    let store = Store::at(temporary.0.join("store"));
    let steps = map_recipes(&store);
    let catalog = steps.iter().find(|step| step.name == catalog::LAYER).unwrap();
    assert_eq!(catalog.code.sources, ["osm-planet", "copernicus-glo-30"]);
    assert!(catalog.outputs.iter().any(|path| path == "LICENSE.txt"));
    let source = steps.iter().find(|step| step.name.starts_with("maps/source/")).unwrap();
    let osm = steps.iter().find(|step| step.name == "maps/osm").unwrap();
    let cells: Vec<_> = steps.iter().filter(|step| step.outputs.iter().any(|path| path == "cells")).collect();
    assert!(cells.len() > 1);
    assert!(cells.iter().any(|step| step.code.crates == ["obc-draw"]));
    assert!(cells.iter().any(|step| step.code.crates == ["obc-network"]));
    let draw = cells.iter().find(|step| step.code.crates == ["obc-draw"]).unwrap();
    let network = cells.iter().find(|step| step.code.crates == ["obc-network"]).unwrap();
    let root = temporary.0.join("checkout");
    recipe_root(&root, &steps);
    copy_tree(&super::root().join("builder/presets"), &root.join("builder/presets"));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let path = root.join("data/sources.toml");
    let original = std::fs::read_to_string(super::root().join("data/sources.toml")).unwrap();
    std::fs::write(&path, &original).unwrap();
    let identity = |step: &Step| step.code.files(&root).unwrap();
    let before = [identity(source), identity(osm), identity(draw), identity(network)];
    let catalog_before = identity(catalog);
    for id in ["osm-planet", "copernicus-glo-30"] {
        let mut registry: toml::Value = toml::from_str(&original).unwrap();
        let record = registry["source"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|source| source["id"].as_str() == Some(id))
            .unwrap();
        record["attribution"] =
            toml::Value::String(format!("{} updated credit", record["attribution"].as_str().unwrap()));
        std::fs::write(&path, toml::to_string(&registry).unwrap()).unwrap();
        assert_eq!([identity(source), identity(osm), identity(draw), identity(network)], before);
        let after = identity(catalog);
        assert_ne!(after, catalog_before);
        let changed: Vec<_> =
            after.keys().filter(|key| after.get(*key) != catalog_before.get(*key)).map(String::as_str).collect();
        assert_eq!(changed, [format!("data/sources.toml#{id}").as_str(), "identity/source-config"]);
    }
    let mut registry: toml::Value = toml::from_str(&original).unwrap();
    for source in registry["source"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .filter(|source| ["osm-planet", "copernicus-glo-30"].contains(&source["id"].as_str().unwrap()))
    {
        source.as_table_mut().unwrap().insert("refresh".into(), toml::Value::Integer(14));
        source["redistribute"] = toml::Value::Boolean(false);
        source.as_table_mut().unwrap().insert("r2_copy".into(), toml::Value::Boolean(false));
    }
    std::fs::write(&path, toml::to_string(&registry).unwrap()).unwrap();
    assert_eq!([identity(source), identity(osm), identity(draw), identity(network)], before);
    assert_eq!(identity(catalog), catalog_before);
}
