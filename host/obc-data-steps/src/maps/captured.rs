//! Captured fixture inputs enter the normal map recipes with their own provenance.

use super::*;
use obc_data::engine::Request;
use obc_data::fixtures::Inputs;
use obc_data::regions::Area;
use obc_data::store::hash_file;

pub(crate) fn recipes(env: &Env, regions: &Regions, store: &Store, inputs: &Inputs) -> Result<Steps, Unplanned> {
    let region = regions.get(&env.region).ok_or_else(|| invalid("fixture region is missing".into()))?;
    let Area::Box { bbox } = &region.area else {
        return Err(invalid("captured fixture inputs require a canonical box".into()));
    };
    let poly = box_poly(bbox);
    let coverage = Coverage::parse_poly(&poly).map_err(Unplanned::Failed)?;
    let obc_data::fixtures::CapturedInput { source, version, files } = &inputs.osm;
    let paths = snapshot_files(store, source, version, &[], files)
        .map_err(Unplanned::Failed)?
        .ok_or_else(|| invalid(format!("import the exact fixture PBF {} before building", inputs.osm_sha256)))?;
    let [path] = paths.values().collect::<Vec<_>>()[..] else {
        return Err(invalid("fixture PBF input must select exactly one file".into()));
    };
    if hash_file(path).map_err(Unplanned::Failed)?.0 != inputs.osm_sha256 {
        return Err(invalid("fixture PBF differs from the bootstrap's stored bytes".into()));
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
        let input = inputs.content.get(collection).ok_or_else(|| {
            invalid(format!("fixture {collection} needs an exact raw capture or an explicitly pinned compiled input"))
        })?;
        content.push(copy_step(
            &content_layer(collection),
            input.input(),
            serde_json::json!({"kind":"historical-compiled","collection":collection}),
            &[collection],
            copy_content,
        ));
    }
    let mut wanted = Vec::new();
    let tile_list = text(env, store, TILE_LIST, &[], &mut wanted)?;
    let glo30 = snapshot_version(env, store, GLO30, &[], &mut wanted)?;
    let land_polygons = snapshot_version(env, store, LAND, &[], &mut wanted)?;
    let (Some(tile_list), Some(glo30), true) = (tile_list, glo30, wanted.is_empty()) else {
        return Err(Unplanned::NeedsFetch(wanted));
    };
    Maps.recipes(
        env,
        regions,
        store,
        obc_osm::OsmiumRunner::default().binding(),
        obc_pack::step::geos_libraries(),
        RecipeInputs { selection, tile_list, land_polygons, glo30, catalog_index: None, content: Some(content) },
    )
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
