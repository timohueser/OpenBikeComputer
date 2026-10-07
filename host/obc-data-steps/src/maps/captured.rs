//! Captured fixture inputs enter the normal map recipes with their own provenance.

use super::*;
use obc_data::engine::Request;
use obc_data::fixtures::{Empty, Inputs};
use obc_data::regions::Area;
use obc_data::store::hash_file;

pub(crate) fn recipes(env: &Env, regions: &Regions, store: &Store, inputs: &Inputs) -> Result<Steps, Unplanned> {
    let region = regions.get(&env.region).ok_or_else(|| invalid("fixture region is missing".into()))?;
    let Area::Box { bbox } = &region.area else {
        return Err(invalid("captured fixture inputs require a canonical box".into()));
    };
    let poly = box_poly(bbox);
    let coverage = Coverage::parse_poly(&poly).map_err(Unplanned::Failed)?;
    let obc_data::fixtures::CapturedInput { source, version, params, files } = &inputs.osm;
    let paths = snapshot_files(store, source, version, params, files)
        .map_err(Unplanned::Failed)?
        .ok_or_else(|| invalid(format!("import the exact fixture PBF {} before building", inputs.osm_sha256)))?;
    let [path] = paths.values().collect::<Vec<_>>()[..] else {
        return Err(invalid("fixture PBF input must select exactly one file".into()));
    };
    if hash_file(path).map_err(Unplanned::Failed)?.0 != inputs.osm_sha256 {
        return Err(invalid("fixture PBF differs from its recorded stored bytes".into()));
    }
    let prepared = copy_step(
        "maps/source/captured",
        inputs.osm.input(),
        serde_json::json!({"poly":poly,"sha256":inputs.osm_sha256}),
        &["source.osm.pbf", "source.poly"],
        copy_osm,
    );
    let selection = crate::region_sources::Selection {
        sources: vec![crate::region_sources::Source {
            id: source.clone(),
            extract: version.clone(),
            poly: String::new(),
            coverage: coverage.clone(),
            prepared: Some(prepared),
        }],
        outlines: vec![coverage.clone()],
        coverage,
        direct: true,
        index: None,
    };
    let mut content = Vec::new();
    for collection in ["landmarks", "peaks"] {
        if let Some(kind) = inputs.empty.get(collection) {
            let kind = match kind {
                Empty::Historical => "historical-empty",
                Empty::Selected => "selected-empty",
            };
            content.push(copy_step(
                &content_layer(collection),
                inputs.osm.input(),
                serde_json::json!({"kind":kind,"collection":collection,"sha256":inputs.osm_sha256}),
                &[collection],
                empty_content,
            ));
            continue;
        }
        let input = inputs.content.get(collection).ok_or_else(|| {
            invalid(format!("fixture {collection} needs an exact raw capture or an explicitly pinned compiled input"))
        })?;
        let raw = collection == "landmarks" && input.source == "fixture-assistant-wiki";
        let mut step = copy_step(
            &content_layer(collection),
            input.input(),
            serde_json::json!({"kind":if raw { "captured-raw" } else { "historical-compiled" },"collection":collection}),
            &[collection],
            if raw { compile_raw_content } else { copy_content },
        );
        if raw {
            step.code.crates.push("obc-pack".into());
            step.code.libraries = obc_pack::step::geos_libraries().map_err(Unplanned::Invalid)?;
        }
        content.push(step);
    }
    let mut wanted = Vec::new();
    let glo30 = env
        .moves
        .get(GLO30)
        .and_then(Option::as_ref)
        .ok_or_else(|| invalid("fixture terrain needs an explicitly selected GLO30 version".into()))?
        .clone();
    let selected_terrain = match &inputs.terrain {
        Some(terrain) => {
            snapshot_files(store, &terrain.source, &terrain.version, &[], &terrain.files)
                .map_err(Unplanned::Failed)?
                .ok_or_else(|| invalid("restore the exact captured terrain files before building".into()))?;
            terrain.clone()
        }
        None => obc_data::fixtures::CapturedInput {
            source: GLO30.into(),
            version: glo30.clone(),
            params: Vec::new(),
            files: Vec::new(),
        },
    };
    let land_polygons = snapshot_version(env, store, LAND, &[], &mut wanted)?;
    if !wanted.is_empty() {
        return Err(Unplanned::NeedsFetch(wanted));
    }
    Maps.recipes(
        env,
        regions,
        store,
        obc_osm::OsmiumRunner::default().binding().map(Some),
        obc_pack::step::geos_libraries(),
        RecipeInputs {
            selection,
            tile_list: String::new(),
            land_polygons,
            glo30,
            catalog_index: None,
            content: Some(content),
            terrain: Some(selected_terrain),
        },
    )
}

/// A captured list is an exact selection, not a declaration that omitted squares are sea.
pub(super) fn bind_terrain(
    env: &Env,
    store: &Store,
    step: &mut Step,
    input: &obc_data::fixtures::CapturedInput,
    cells: &[CellId],
) -> Result<(), Unplanned> {
    let required: BTreeSet<_> = cells
        .iter()
        .flat_map(|cell| obc_dem::fetch::tiles_for(obc_bake::terrain::source_bbox([*cell]).expect("a cell has a box")))
        .map(|tile| tile.file_name())
        .collect();
    let missing: Vec<_> = required.iter().filter(|file| !input.files.contains(file)).cloned().collect();
    step.inputs = vec![Input::Snapshot {
        source: input.source.clone(),
        version: input.version.clone(),
        params: Vec::new(),
        files: required.iter().filter(|file| input.files.contains(file)).cloned().collect(),
    }];
    if !missing.is_empty() {
        let Some(Some(version)) = env.moves.get(GLO30) else {
            return Err(invalid(format!("captured terrain lacks full required coverage: {}; select the exact historical `{GLO30}` version in data/env/fixtures.toml before preparation", missing.join(", "))));
        };
        let params: Vec<_> = missing.iter().map(|name| ("tile".into(), name.trim_end_matches(".tif").into())).collect();
        let files = match read(env, store, GLO30, &params).map_err(Unplanned::Failed)? {
            Ok(files) => files,
            Err(fetch) => return Err(Unplanned::NeedsFetch(vec![fetch])),
        };
        if missing.iter().any(|name| !files.contains_key(name)) {
            return Err(invalid(format!("historical terrain still lacks required raw TIFFs: {}; restore this exact source version, never substitute sea or newest", missing.join(", "))));
        }
        step.inputs.push(Input::Snapshot { source: GLO30.into(), version: version.clone(), params, files: missing });
        step.code.sources.push(GLO30.into());
    }
    if matches!(&step.inputs[0], Input::Snapshot { files, .. } if files.is_empty()) {
        step.inputs.remove(0);
    }
    step.code.crates.push("obc-data-steps".into());
    step.code.sources.push(input.source.clone());
    step.options["dataset"] = serde_json::json!("selected-glo-30");
    step.run = Run::Rust(captured_terrain);
    Ok(())
}

fn captured_terrain(request: &Request) -> Result<(), String> {
    let mut files = BTreeMap::new();
    for selected in request.snapshots.values() {
        for (name, path) in selected {
            if files.insert(name.clone(), path.clone()).is_some() {
                return Err("captured terrain selects a raw TIFF twice".into());
            }
        }
    }
    if files.is_empty() {
        return Err("captured terrain has no raw TIFFs".into());
    }
    let declared = Request {
        step: request.step.clone(),
        snapshots: [(GLO30.into(), files)].into(),
        layers: request.layers.clone(),
        layer_files: request.layer_files.clone(),
        options: request.options.clone(),
        libraries: request.libraries.clone(),
        output: request.output.clone(),
        metrics: request.metrics.clone(),
    };
    obc_dem::step::terrain(&declared)
}

fn empty_content(request: &Request) -> Result<(), String> {
    use obc_pack::landmarks::{peaks::PeakContent, Content, Counts};
    let collection = request.options["collection"].as_str().ok_or("empty fixture collection is missing")?;
    let digest = request.options["sha256"].as_str().ok_or("empty fixture input identity is missing")?.to_string();
    let value = match collection {
        "landmarks" => serde_json::to_value(Content {
            schema: 2,
            input_sha256: digest.clone(),
            policy_sha256: digest.clone(),
            category_policy_sha256: digest,
            languages: Vec::new(),
            source_coverage: serde_json::Value::Null,
            counts: Counts::default(),
            candidate_qids: Vec::new(),
            records: Vec::new(),
            omissions: Vec::new(),
            photo_requests: Vec::new(),
        }),
        "peaks" => serde_json::to_value(PeakContent {
            schema: 1,
            collection: "peaks".into(),
            input_sha256: digest.clone(),
            policy_sha256: digest,
            languages: Vec::new(),
            source_coverage: serde_json::Value::Null,
            counts: Counts::default(),
            associations: Vec::new(),
            records: Vec::new(),
            omissions: Vec::new(),
            photo_requests: Vec::new(),
        }),
        _ => return Err("unknown empty fixture collection".into()),
    }
    .map_err(|e| e.to_string())?;
    let output = request.output.join(collection);
    std::fs::create_dir_all(&output).map_err(|e| e.to_string())?;
    std::fs::write(
        output.join(if collection == "peaks" { "peaks.json" } else { "content.json" }),
        serde_json::to_vec(&value).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

fn copy_step(
    name: &str,
    input: Input,
    options: serde_json::Value,
    outputs: &[&str],
    run: fn(&Request) -> Result<(), String>,
) -> Step {
    let sources = match &input {
        Input::Snapshot { source, .. } => vec![source.clone()],
        _ => Vec::new(),
    };
    Step {
        name: name.into(),
        inputs: vec![input],
        options,
        code: Code { crates: vec!["obc-data-steps".into()], sources, ..Default::default() },
        outputs: outputs.iter().map(|path| path.to_string()).collect(),
        run: Run::Rust(run),
        client: Client::None,
    }
}

fn copy_osm(request: &Request) -> Result<(), String> {
    let paths = request.snapshots.values().flat_map(|files| files.values()).collect::<Vec<_>>();
    let [path] = paths[..] else { return Err("captured PBF request does not select one file".into()) };
    if hash_file(path)?.0 != request.options["sha256"].as_str().ok_or("captured PBF lacks a digest")? {
        return Err("captured PBF changed before preparation".into());
    }
    std::fs::copy(path, request.output.join("source.osm.pbf")).map_err(|e| e.to_string())?;
    std::fs::write(request.output.join("source.poly"), request.options["poly"].as_str().ok_or("missing box polygon")?)
        .map_err(|e| e.to_string())
}

fn compile_raw_content(request: &Request) -> Result<(), String> {
    let files = request
        .snapshots
        .values()
        .flat_map(|selected| selected.iter())
        .map(|(name, path)| (name.clone(), path.clone()))
        .collect();
    let view = request.output.with_file_name("fixture-content-input");
    obc_pack::step::copied_view(&files, &view)?;
    obc_pack::landmarks::compile(
        &view.join("manifest.json"),
        &view.join("regions.geojson"),
        &request.output.join("landmarks"),
        false,
    )
    .map(drop)
}

fn copy_content(request: &Request) -> Result<(), String> {
    let collection = request.options["collection"].as_str().ok_or("compiled fixture input lacks its collection")?;
    let document = match collection {
        "landmarks" => "content.json",
        "peaks" => "peaks.json",
        _ => return Err("unknown compiled fixture collection".into()),
    };
    let files = request.snapshots.values().flat_map(|files| files.iter()).collect::<BTreeMap<_, _>>();
    if !files.keys().any(|path| path.as_str() == document) {
        return Err(format!("pinned fixture {collection} input lacks {document}"));
    }
    for (name, path) in files {
        obc_data::fixtures::relative(name)?;
        let destination = request.output.join(collection).join(name);
        std::fs::create_dir_all(destination.parent().expect("content file has a parent")).map_err(|e| e.to_string())?;
        std::fs::copy(path, destination).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_raw_content_uses_the_shared_writer_and_refuses_changed_source_bytes() {
        let temporary = super::super::tests::temp("legacy-fixture-content");
        std::fs::create_dir_all(&temporary.0).unwrap();
        let response = temporary.0.join("source.json");
        std::fs::write(&response, b"{}").unwrap();
        let manifest = temporary.0.join("manifest.json");
        std::fs::write(
            &manifest,
            serde_json::to_vec(&serde_json::json!({
                "schema":1,"places":[],"sources":[{"path":"source.json", "bytes":2,
                    "sha256":hash_file(&response).unwrap().0,"url":"https://en.wikipedia.org/w/api.php"}]
            }))
            .unwrap(),
        )
        .unwrap();
        let boundary = temporary.0.join("regions.geojson");
        std::fs::write(
            &boundary,
            serde_json::to_vec(&serde_json::json!({
                "type":"FeatureCollection", "features":[{"type":"Feature","geometry":{
                    "type":"Polygon","coordinates":[[[-9.9,51.43],[-9.5,51.43],[-9.5,51.65],[-9.9,51.65],[-9.9,51.43]]]
                }}]
            }))
            .unwrap(),
        )
        .unwrap();
        let mut request = Request {
            step: "maps/landmark-content".into(),
            snapshots: [(
                "fixture-assistant-wiki".into(),
                [
                    ("source.json".into(), response.clone()),
                    ("manifest.json".into(), manifest),
                    ("regions.geojson".into(), boundary),
                ]
                .into(),
            )]
            .into(),
            layers: BTreeMap::new(),
            layer_files: BTreeMap::new(),
            options: serde_json::json!({}),
            libraries: Vec::new(),
            output: temporary.0.join("out"),
            metrics: temporary.0.join("metrics.json"),
        };
        compile_raw_content(&request).unwrap();
        let compiled: serde_json::Value =
            serde_json::from_slice(&std::fs::read(request.output.join("landmarks/content.json")).unwrap()).unwrap();
        assert_eq!(compiled["schema"], 2);
        assert!(compiled["records"].as_array().unwrap().is_empty());
        assert!(!request.output.with_file_name("fixture-content-input").join("recipe.json").exists());
        std::fs::write(&response, b"[]").unwrap();
        request.output = temporary.0.join("retry/output");
        let error = compile_raw_content(&request).unwrap_err();
        assert!(error.contains("source checksum changed"), "{error}");
        assert!(!request.output.join("landmarks/content.json").exists());
    }

    #[test]
    fn captured_terrain_requires_the_full_cell_window_and_keeps_its_actual_snapshot() {
        let cell = CellId::containing(V1_CELL_LOG2.into(), 46_600_000, 8_300_000);
        let files: Vec<_> = obc_dem::fetch::tiles_for(obc_bake::terrain::source_bbox([cell]).unwrap())
            .iter()
            .map(|tile| tile.file_name())
            .collect();
        let mut input = obc_data::fixtures::CapturedInput {
            source: "fixture-assistant-terrain".into(),
            version: "a".repeat(64),
            params: Vec::new(),
            files: files.clone(),
        };
        let mut step = terrain(LeafId { i: 0, j: 0 }, &[cell], &HashSet::new(), "unused", None);
        let temporary = super::super::tests::temp("captured-terrain-window");
        let store = Store::at(&temporary.0);
        let env = Env::default();
        bind_terrain(&env, &store, &mut step, &input, &[cell]).unwrap();
        let [Input::Snapshot { source, version, params, files: selected }] = &step.inputs[..] else {
            panic!("one captured snapshot")
        };
        assert_eq!(source, &input.source);
        assert_eq!(version, &input.version);
        assert!(params.is_empty());
        assert_eq!(selected, &files);
        assert_eq!(step.options["dataset"], "selected-glo-30");
        let missing = input.files.pop().unwrap();
        assert!(format!("{:?}", bind_terrain(&env, &store, &mut step, &input, &[cell]).unwrap_err()).contains(&missing));
        let env = Env { moves: [(GLO30.into(), Some("2022-05-09".into()))].into(), ..Default::default() };
        let Unplanned::NeedsFetch(wanted) = bind_terrain(&env, &store, &mut step, &input, &[cell]).unwrap_err() else {
            panic!("only the missing raw tile requires exact-version acquisition")
        };
        assert_eq!(wanted.len(), 1);
        assert_eq!(wanted[0].version.as_deref(), Some("2022-05-09"));
        assert_eq!(wanted[0].params, vec![("tile".into(), missing.trim_end_matches(".tif").into())]);
    }

    #[test]
    fn captured_input_checks_stored_bytes_and_keeps_the_box_as_explicit_recipe_data() {
        let temporary = super::super::tests::temp("captured-fixture");
        std::fs::create_dir_all(&temporary.0).unwrap();
        let source = temporary.0.join("west-cork.osm.pbf");
        std::fs::write(&source, b"stored crop bytes").unwrap();
        let output = temporary.0.join("prepared");
        std::fs::create_dir(&output).unwrap();
        let poly = "west-cork\n0\n -9.9 51.43\n -9.5 51.43\n -9.5 51.65\n -9.9 51.65\n -9.9 51.43\nEND\nEND\n";
        let mut request = Request {
            step: "maps/source/captured".into(),
            snapshots: [("fixture-osm".into(), [("west-cork.osm.pbf".into(), source.clone())].into())].into(),
            layers: BTreeMap::new(),
            layer_files: BTreeMap::new(),
            options: serde_json::json!({"poly":poly,"sha256":obc_data::store::sha256_hex(b"upstream country bytes")}),
            libraries: Vec::new(),
            output: output.clone(),
            metrics: temporary.0.join("metrics.json"),
        };
        assert!(copy_osm(&request).unwrap_err().contains("changed"));
        assert!(!output.join("source.osm.pbf").exists());
        request.options["sha256"] = serde_json::json!(hash_file(&source).unwrap().0);
        copy_osm(&request).unwrap();
        assert_eq!(std::fs::read(output.join("source.osm.pbf")).unwrap(), b"stored crop bytes");
        assert_eq!(std::fs::read_to_string(output.join("source.poly")).unwrap(), poly);
    }
}
