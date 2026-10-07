//! Bootstrap archives contribute exact source snapshots, never current producer receipts.

use super::*;
use crate::engine::{Code, Python};
use crate::fetch::http::{Expect, Http};
use crate::sources::Source;
use crate::store::{FileRecord, Snapshot};

pub(crate) fn packaging(root: &Path) -> Result<BTreeMap<String, String>, String> {
    materializer().files(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::tests::{fixture, write};

    #[test]
    fn reviewed_sealer_changes_refuse_before_materializing_any_archive() {
        let fixture = fixture("reviewed-fixture-sealer");
        let root = fixture.root();
        let real = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for path in ["pyproject.toml", "uv.lock", "tools/data_fixtures.py", "tools/fixtures.py"] {
            write(&root.join(path), &std::fs::read_to_string(real.join(path)).unwrap());
        }
        let reviewed = packaging(&root).unwrap();
        write(&root.join("tools/data_fixtures.py"), "raise RuntimeError('must not execute changed sealer')\n");
        let archive = root.join("unreviewed.tar.gz");
        let error = materialize(
            &root,
            &["seal".into(), "ride".into(), root.to_string_lossy().into(), archive.to_string_lossy().into()],
            Some(&reviewed),
        )
        .unwrap_err();
        assert!(error.contains("changed after review"), "{error}");
        assert!(!archive.exists());
    }
}

fn materializer() -> Code {
    Code {
        files: vec!["tools/data_fixtures.py".into(), "tools/fixtures.py".into()],
        python: Some(Python::default()),
        ..Default::default()
    }
}

pub(crate) fn materialize(
    root: &Path,
    args: &[String],
    expected: Option<&BTreeMap<String, String>>,
) -> Result<serde_json::Value, String> {
    let code = materializer();
    let identity = code.files(root)?;
    if expected.is_some_and(|expected| expected != &identity) {
        return Err("fixture sealer or Python changed after review; make a new plan".into());
    }
    let argv = [vec!["python".into(), "-m".into(), "tools.data_fixtures".into()], args.to_vec()].concat();
    let output = code.command(root, &argv)?.output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().into());
    }
    if code.files(root)? != identity {
        return Err("fixture materializer changed during execution; prepare and review again".into());
    }
    serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())
}

pub(crate) fn archives(
    root: &Path,
    store: &Store,
    http: &Http,
    catalog: &Catalog,
    bootstrap: &Bootstrap,
    package: &Package,
    sources: &[Source],
) -> Result<(), String> {
    let mut packages = bootstrap.source_packages.clone();
    for (source, version) in &package.sources {
        if let Some(id) = source.strip_prefix("fixture-") {
            digest(version)?;
            if packages.get(id).is_some_and(|original| original != version) {
                return Err(format!("{source}: declared archive differs from the original bootstrap"));
            }
            packages.insert(id.into(), version.clone());
        }
    }
    for (id, hash) in &packages {
        let source = format!("fixture-{id}");
        if crate::engine::snapshot_files(store, &source, hash, &[], &[])?.is_some() {
            continue;
        }
        let registered = sources.iter().find(|s| s.id == source).ok_or_else(|| {
            format!("register captured fixture source `{source}` with its confirmed licence before preparation")
        })?;
        if registered.licence.is_none() {
            return Err(format!("{source}: capture redistribution terms are unconfirmed; review source notices first"));
        }
        let url = format!("{}packages/{hash}.tar.gz", catalog.base_url);
        let _lock = Http::lock(store, &url)?;
        let archive = http.download(store, &url, &Expect { sha256: Some(hash), ..Default::default() })?;
        if hash_file(&archive.object)?.0 != *hash {
            return Err(format!("bootstrap archive {hash} changed"));
        }
        let scratch = crate::r2::Scratch::new()?;
        let directory = scratch.0.join("input");
        let imported = materialize(
            root,
            &[
                "import".into(),
                id.clone(),
                archive.object.to_string_lossy().into(),
                hash.clone(),
                directory.to_string_lossy().into(),
            ],
            None,
        )?;
        let mut files = Vec::new();
        for file in imported["files"].as_array().ok_or("fixture archive has no file inventory")? {
            let name = file["path"].as_str().ok_or("fixture archive has no file path")?;
            relative(name)?;
            let path = directory.join(name);
            let (sha256, size) = hash_file(&path)?;
            let copy = store.partial(&format!("fixture-{sha256}"));
            std::fs::create_dir_all(copy.parent().expect("partial has a parent")).map_err(|e| e.to_string())?;
            std::fs::copy(path, &copy).map_err(|e| e.to_string())?;
            store.insert(&copy, &sha256)?;
            files.push(FileRecord {
                name: name.into(),
                url: format!("{url}#{name}"),
                size,
                sha256,
                retrieved: crate::date::timestamp(crate::date::now()),
            });
        }
        store.put_snapshot(&Snapshot { source, version: hash.clone(), files })?;
    }
    Ok(())
}

pub(crate) fn inputs(root: &Path, store: &Store, package: &Package, bootstrap: &Bootstrap) -> Result<Inputs, String> {
    let osm = match &bootstrap.source_pbf {
        Some(osm) => Captured { path: osm.path.clone(), bytes: osm.bytes, sha256: osm.sha256.clone() },
        None => {
            let manifest: serde_json::Value = serde_json::from_slice(
                &std::fs::read(root.join("fixtures/sources/ride-assistant/assistant-osm.json"))
                    .map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            let entry = manifest["sources"]
                .as_array()
                .ok_or("captured OSM manifest has no sources")?
                .iter()
                .find(|entry| entry["path"].as_str() == Some(&package.osm))
                .ok_or("bootstrap has no exact captured PBF")?;
            Captured {
                path: package.osm.clone(),
                bytes: entry["bytes"].as_u64().ok_or("captured PBF has no size")?,
                sha256: entry["sha256"].as_str().ok_or("captured PBF has no digest")?.into(),
            }
        }
    };
    digest(&osm.sha256)?;
    let mut captured = None;
    let mut content = BTreeMap::new();
    for (id, version) in &bootstrap.source_packages {
        let source = format!("fixture-{id}");
        let Some(snapshot) = store.snapshot(&source, version)? else { continue };
        if snapshot.files.iter().any(|f| f.name == package.osm && f.sha256 == osm.sha256 && f.size == osm.bytes) {
            captured = Some(CapturedInput {
                source: source.clone(),
                version: version.clone(),
                files: vec![package.osm.clone()],
            });
        }
        for (collection, name) in [("landmarks", "content.json"), ("peaks", "peaks.json")] {
            if snapshot.files.iter().any(|f| f.name == name) {
                if content
                    .insert(
                        collection.into(),
                        CapturedInput { source: source.clone(), version: version.clone(), files: Vec::new() },
                    )
                    .is_some()
                {
                    return Err(format!("bootstrap selects conflicting {collection} inputs"));
                }
            }
        }
    }
    if captured.is_none() {
        for snapshot in store.snapshots("geofabrik-extracts")? {
            if let Some(file) = snapshot.files.iter().find(|f| f.sha256 == osm.sha256 && f.size == osm.bytes) {
                captured = Some(CapturedInput {
                    source: snapshot.source.clone(),
                    version: snapshot.version.clone(),
                    files: vec![file.name.clone()],
                });
                break;
            }
        }
    }
    let record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join(&package.bootstrap)).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    if let Some(version) = package.sources.get("land-polygons") {
        if let Some(snapshot) = store.snapshot("land-polygons", version)? {
            for expected in record["land_source"]["files"].as_array().into_iter().flatten() {
                if !snapshot.files.iter().any(|file| {
                    expected["path"].as_str() == Some(&file.name)
                        && expected["sha256"].as_str() == Some(&file.sha256)
                        && expected["bytes"].as_u64() == Some(file.size)
                }) {
                    return Err("historical land polygons differ from the original extracted file record".into());
                }
            }
        }
    }
    let mut historical = BTreeMap::new();
    if let Some(version) = record["terrain"]["source_package_sha256"].as_str() {
        let source = "fixture-sim-grimsel";
        let name = "grimsel.obcd";
        let snapshot = store.snapshot(source, version)?.ok_or_else(|| {
            format!("import exact historical terrain sidecar archive {version} before fixture preparation")
        })?;
        if !snapshot.files.iter().any(|file| {
            file.name == name
                && record["terrain"]["sha256"].as_str() == Some(&file.sha256)
                && record["terrain"]["bytes"].as_u64() == Some(file.size)
        }) {
            return Err("historical terrain sidecar differs from its original record".into());
        }
        historical.insert(
            name.into(),
            CapturedInput { source: source.into(), version: version.into(), files: vec![name.into()] },
        );
    }
    let empty = [
        ("landmarks", record["map"]["landmark_records"].as_u64() == Some(0)),
        ("peaks", record["map"]["peak_associations"].as_array().is_some_and(Vec::is_empty)),
    ]
    .into_iter()
    .filter(|(name, absent)| *absent && !content.contains_key(*name))
    .map(|(name, _)| name.into())
    .collect();
    let terrain_version =
        bootstrap.source_packages.get("assistant-terrain").or_else(|| package.sources.get("fixture-assistant-terrain"));
    let terrain = terrain_version
        .map(|version| -> Result<Option<CapturedInput>, String> {
            let source = "fixture-assistant-terrain";
            let Some(snapshot) = store.snapshot(source, version)? else { return Ok(None) };
            let manifest: serde_json::Value = serde_json::from_slice(
                &std::fs::read(root.join("fixtures/sources/ride-assistant/assistant-terrain.json"))
                    .map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            let mut files = Vec::new();
            for entry in manifest["sources"].as_array().ok_or("captured terrain manifest has no sources")? {
                let name = entry["path"].as_str().ok_or("captured terrain has no file path")?;
                if entry["transform"] != "none"
                    || !snapshot.files.iter().any(|file| {
                        file.name == name
                            && entry["sha256"].as_str() == Some(&file.sha256)
                            && entry["bytes"].as_u64() == Some(file.size)
                    })
                {
                    return Err(format!("captured terrain `{name}` differs from its exact original raw TIFF record"));
                }
                files.push(name.into());
            }
            if files.is_empty() {
                return Err("captured terrain archive has no raw TIFFs".into());
            }
            Ok(Some(CapturedInput { source: source.into(), version: version.clone(), files }))
        })
        .transpose()?
        .flatten();
    Ok(Inputs {
        historical,
        terrain,
        osm: captured.ok_or_else(|| {
            format!(
                "import exact captured PBF {} for {}; a latest extract is not a substitute",
                osm.sha256, package.region
            )
        })?,
        osm_sha256: osm.sha256,
        content,
        empty,
    })
}
