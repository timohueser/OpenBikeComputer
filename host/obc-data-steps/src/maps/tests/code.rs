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

fn recipe_root(target: &Path) {
    let repository = root();
    let mut manifest: toml::Value = std::fs::read_to_string(repository.join("Cargo.toml")).unwrap().parse().unwrap();
    let members = ["host/obc-osm", "host/obc-data", "host/obc-pack", "firmware/obc-formats", "firmware/obc-ports"];
    manifest["workspace"]["members"] =
        toml::Value::Array(members.into_iter().map(|name| toml::Value::String(name.into())).collect());
    std::fs::create_dir_all(target).unwrap();
    std::fs::write(target.join("Cargo.toml"), toml::to_string(&manifest).unwrap()).unwrap();
    for name in ["host/obc-osm", "firmware/obc-formats", "firmware/obc-ports"] {
        std::fs::create_dir_all(target.join(name)).unwrap();
        std::fs::copy(repository.join(name).join("Cargo.toml"), target.join(name).join("Cargo.toml")).unwrap();
        copy_tree(&repository.join(name).join("src"), &target.join(name).join("src"));
    }
    for name in ["obc-data", "obc-pack"] {
        let directory = target.join("host").join(name);
        std::fs::create_dir_all(directory.join("src")).unwrap();
        std::fs::write(
            directory.join("Cargo.toml"),
            format!("[package]\nname='{name}'\nversion='0.1.0'\nedition='2021'\n"),
        )
        .unwrap();
        std::fs::write(directory.join("src/lib.rs"), "pub const VALUE: u32 = 1;\n").unwrap();
    }
    std::fs::write(target.join("host/obc-pack/src/nav.rs"), "pub const COST: u32 = 1;\n").unwrap();
    let output = std::process::Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .args(["generate-lockfile", "--offline"])
        .current_dir(target)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
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

fn authored_network(request: &Request) -> Result<(), String> {
    std::fs::write(request.output.join("network.obcm"), "network").map_err(|e| e.to_string())
}

fn map_recipes(store: &Store) -> Vec<Step> {
    let (env, regions) = freiburg(store);
    with_osm(store);
    with_captures(store, "1");
    without_models(store, &env, &regions);
    map_steps(&root(), &env, &regions, store).unwrap().steps
}

#[test]
fn navigation_edits_reuse_the_actual_source_and_osm_step_recipes() {
    let temporary = temp("source-code-scope");
    let store = Store::at(temporary.0.join("store"));
    let mut steps = map_recipes(&store);
    steps.retain(|step| step.name.starts_with("maps/source/") || step.name == "maps/osm");
    assert_eq!(steps.len(), 2);
    assert!(steps.iter().all(|step| step.code.crates == ["obc-osm"]));
    steps.iter_mut().find(|step| step.name == "maps/osm").unwrap().run = Run::Rust(authored_osm);
    steps.push(Step {
        name: "maps/network".into(),
        inputs: vec![Input::layer("maps/osm")],
        options: serde_json::json!({}),
        code: Code { crates: vec!["obc-pack".into()], ..Default::default() },
        outputs: vec!["network.obcm".into()],
        run: Run::Rust(authored_network),
        client: Client::None,
    });
    let root = temporary.0.join("checkout");
    recipe_root(&root);
    let files = obc_data::engine::code::files(&root, &steps[0].code).unwrap();
    assert!(files.keys().any(|path| path.ends_with("obc-osm/src/step.rs")));
    assert!(files.keys().all(|path| !path.contains("obc-pack") && !path.contains("obc-bake")));
    let http = Http::new();
    let context =
        Context { root: &root, store: &store, sources: &[], http: &http, copies: None, limits: Limits::machine() };
    let initial = plan(&store, &root, &steps).unwrap();
    assert_eq!(builds(&initial).len(), 3);
    let mut run = RunLog::create(&store, "build scoped source").unwrap();
    run.build(&context, &steps, &initial).unwrap();
    run.finish(None).unwrap();
    assert!(plan(&store, &root, &steps).unwrap().builds().next().is_none());
    std::fs::write(root.join("host/obc-pack/src/nav.rs"), "pub const COST: u32 = 2;\n").unwrap();
    assert_eq!(builds(&plan(&store, &root, &steps).unwrap()), ["maps/network"]);
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
    assert!(cells.iter().all(|step| step.code == cells[0].code));
    let cell = cells[0];
    let root = temporary.0.join("checkout");
    recipe_root(&root);
    copy_tree(&super::root().join("builder/presets"), &root.join("builder/presets"));
    std::fs::create_dir(root.join("data")).unwrap();
    let path = root.join("data/sources.toml");
    let original = std::fs::read_to_string(super::root().join("data/sources.toml")).unwrap();
    std::fs::write(&path, &original).unwrap();
    let identity = |step: &Step| obc_data::engine::code::files(&root, &step.code).unwrap();
    let before = [identity(source), identity(osm), identity(cell)];
    let catalog_before = identity(catalog);
    for id in ["osm-planet", "copernicus-glo-30"] {
        let mut registry: toml::Value = original.parse().unwrap();
        let record = registry["source"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|source| source["id"].as_str() == Some(id))
            .unwrap();
        record["attribution"] =
            toml::Value::String(format!("{} updated credit", record["attribution"].as_str().unwrap()));
        std::fs::write(&path, toml::to_string(&registry).unwrap()).unwrap();
        assert_eq!([identity(source), identity(osm), identity(cell)], before);
        let after = identity(catalog);
        assert_ne!(after, catalog_before);
        let changed: Vec<_> = after.keys().filter(|key| after.get(*key) != catalog_before.get(*key)).collect();
        assert_eq!(changed, [&format!("data/sources.toml#{id}")]);
    }
    let mut registry: toml::Value = original.parse().unwrap();
    for source in registry["source"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .filter(|source| ["osm-planet", "copernicus-glo-30"].contains(&source["id"].as_str().unwrap()))
    {
        source["refresh"] = toml::Value::Integer(14);
        source["redistribute"] = toml::Value::Boolean(false);
        source["r2_copy"] = toml::Value::Boolean(false);
    }
    std::fs::write(&path, toml::to_string(&registry).unwrap()).unwrap();
    assert_eq!([identity(source), identity(osm), identity(cell)], before);
    assert_eq!(identity(catalog), catalog_before);
}
