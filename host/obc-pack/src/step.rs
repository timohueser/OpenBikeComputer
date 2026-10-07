//! Compiled landmark and peak content, and per-cell article artifacts.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use obc_data::engine::{view, Request};
use serde_json::Value;
use sha2::{Digest, Sha256};

use obc_map_core::config::{Config, CELL_SCHEMA as SCHEMA};
use obc_map_core::grid::CellId;
use obc_map_core::progress::{CancelToken, Progress};

/// The sources of one landmark or peak capture.
pub const CAPTURES: [&str; 3] = ["wikidata", "wikipedia", "commons"];

/// The digest of the code that makes the boundary and the candidates or the summits, which a
/// capture pins: the param `code=` of a capture, so a capture with other code is another request.
pub fn capture_code() -> String {
    let code = [
        include_str!("catalog/boundary.rs"),
        include_str!("../../obc-pbf/src/area.rs"),
        include_str!("../../obc-places/src/osm.rs"),
        include_str!("../../obc-places/src/lib.rs"),
        include_str!("../../obc-places/src/metadata.rs"),
        include_str!("../../obc-places/src/name.rs"),
        include_str!("landmarks/discover.rs"),
        include_str!("landmarks/peaks.rs"),
    ];
    Sha256::digest(code.concat()).iter().take(8).map(|byte| format!("{byte:02x}")).collect()
}

/// The compiled landmarks of a region: `landmarks/content.json` and its photos, from the capture
/// files, the `.poly` and the `.osm.pbf` that the capture read. The capture keeps no copy of the
/// boundary or of the candidates, which are OSM data, so the step makes them again, byte for byte.
pub fn landmark_content(request: &Request) -> Result<(), String> {
    let (view, boundary, extract) = capture_view(request)?;
    crate::landmarks::discover::discover(&extract, &view.join("candidates.json"))?;
    check_pins(&view, &boundary, "candidates")?;
    crate::landmarks::compile(&view.join("manifest.json"), &boundary, &request.output.join("landmarks"), false)
        .map(drop)
}

/// The compiled peaks of a region: `peaks/peaks.json` and its photos, as [`landmark_content`]
/// makes the landmarks, with the summits in place of the candidates.
pub fn peak_content(request: &Request) -> Result<(), String> {
    let (view, boundary, extract) = capture_view(request)?;
    crate::landmarks::peaks::discover(&extract, &boundary, &view.join("summits.json"))?;
    check_pins(&view, &boundary, "summits")?;
    crate::landmarks::peaks::compile(&view.join("manifest.json"), &boundary, &request.output.join("peaks"), false)
        .map(drop)
}

/// Refuse a capture whose recipe pinned another boundary, or other `candidates` or `summits`, than
/// the step made again: the code that makes them changed since the capture, and a new capture
/// with this code is the fix.
fn check_pins(view: &Path, boundary: &Path, made: &str) -> Result<(), String> {
    let read = |path: &Path| std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()));
    let recipe: Value = serde_json::from_slice(&read(&view.join("recipe.json"))?)
        .map_err(|e| format!("the recipe of the capture: {e}"))?;
    for (name, path) in [("boundary", boundary.to_path_buf()), (made, view.join(format!("{made}.json")))] {
        let digest: String = Sha256::digest(read(&path)?).iter().map(|byte| format!("{byte:02x}")).collect();
        if recipe[format!("{name}_sha256")].as_str() != Some(digest.as_str()) {
            return Err(format!(
                "the capture of `{}` is out of date for this code: the {name} that it pinned and the {name} that \
                 this code makes are not the same. Plan with `--move {}`",
                CAPTURES[0], CAPTURES[0]
            ));
        }
    }
    Ok(())
}

/// The capture files as the capture wrote them, the boundary from the `.poly`, and the `.osm.pbf`.
fn capture_view(request: &Request) -> Result<(PathBuf, PathBuf, PathBuf), String> {
    let mut capture = BTreeMap::new();
    let mut other = Vec::new();
    for (source, files) in &request.snapshots {
        for (name, object) in files {
            if !CAPTURES.contains(&source.as_str()) {
                other.push((name, object));
                continue;
            }
            // A capture file is `#<query>/<path in the capture>`.
            let path = name.strip_prefix('#').and_then(|name| name.split_once('/')).map(|(_, path)| path);
            capture.insert(path.ok_or(format!("{source}: {name} is not a capture file"))?.to_string(), object.clone());
        }
    }
    let one = |suffix: &str| match other.iter().filter(|(name, _)| name.ends_with(suffix)).collect::<Vec<_>>()[..] {
        [(_, object)] => Ok((*object).clone()),
        ref found => Err(format!("the step reads {} {suffix} files, not one", found.len())),
    };
    let (poly, extract) = (one(".poly")?, one(".osm.pbf")?);
    let view = request.output.with_file_name("view");
    copied_view(&capture, &view)?;
    let boundary = request.output.with_file_name("boundary.geojson");
    let poly = std::fs::read_to_string(&poly).map_err(|e| format!("{}: {e}", poly.display()))?;
    std::fs::write(&boundary, crate::catalog::boundary::geojson(&poly)?)
        .map_err(|e| format!("{}: {e}", boundary.display()))?;
    Ok((view, boundary, extract))
}

/// The landmark artifacts of one leaf: `landmarks/<i>/<j>.bin` for each cell of the option `cells`
/// (`[i, j]` on the grid of the option `cell_log2`) that owns a landmark of the compiled content,
/// joined to the OSM objects of the one `.osm.pbf` that name it.
pub fn landmarks(request: &Request) -> Result<(), String> {
    let cells = artifact_cells(request)?;
    let files: BTreeMap<&String, &PathBuf> = request.layers.values().flatten().collect();
    let pbfs: Vec<String> = files
        .iter()
        .filter(|(path, _)| path.ends_with(".osm.pbf"))
        .map(|(_, object)| object.to_string_lossy().into_owned())
        .collect();
    if pbfs.len() != 1 {
        return Err(format!("the step reads {} .osm.pbf files, not one", pbfs.len()));
    }
    let content = content_views(request, "landmarks", crate::landmarks::CONTENT_DOC)?;
    let progress = Progress::new(CancelToken::new(), |_, line| eprintln!("{line}"));
    let places = obc_places::osm::harvest(&pbfs, &Config::parse(SCHEMA)?, &progress)?;
    let artifacts = crate::landmark_map::artifacts(&content, &places.links, &cells)?;
    write_artifacts(request, "landmarks", artifacts)
}

/// The peak artifacts of one leaf: `peaks/<i>/<j>.bin` for each cell of the option `cells` that
/// owns the summit node of an association of the compiled peaks.
pub fn peaks(request: &Request) -> Result<(), String> {
    let cells = artifact_cells(request)?;
    let content = content_views(request, "peaks", "peaks.json")?;
    write_artifacts(request, "peaks", crate::peak_map::artifacts(&content, &cells)?)
}

/// The cells of the options `cell_log2` and `cells`.
fn artifact_cells(request: &Request) -> Result<Vec<CellId>, String> {
    let log2 = request.options["cell_log2"].as_u64().ok_or("option `cell_log2` is not a number")?;
    let log2 = u32::try_from(log2).map_err(|_| "option `cell_log2` is too large")?;
    let cells = request.options["cells"].as_array().ok_or("option `cells` is not a list")?;
    cells
        .iter()
        .map(|cell| match cell.as_array().map(Vec::as_slice) {
            Some([i, j]) => {
                CellId::new(log2, i.as_i64().ok_or("a cell is not [i, j]")?, j.as_i64().ok_or("a cell is not [i, j]")?)
            }
            _ => Err("a cell is not [i, j]".into()),
        })
        .collect()
}

/// The compiled content of the layer files below `dir`, linked into a directory beside the output,
/// and the path of its document `doc` there.
fn content_views(request: &Request, dir: &str, doc: &str) -> Result<Vec<PathBuf>, String> {
    let mut documents = Vec::new();
    for (index, files) in request.layers.values().enumerate() {
        let content: BTreeMap<String, PathBuf> = files
            .iter()
            .filter(|(path, _)| path.starts_with(&format!("{dir}/")))
            .map(|(path, object)| (path.clone(), object.clone()))
            .collect();
        if content.is_empty() {
            continue;
        }
        if !content.contains_key(&format!("{dir}/{doc}")) {
            return Err(format!("content layer has no {dir}/{doc}"));
        }
        let view = request.output.with_file_name("views").join(index.to_string());
        copied_view(&content, &view)?;
        documents.push(view.join(dir).join(doc));
    }
    if documents.is_empty() {
        return Err(format!("the step has no {dir} content inputs"));
    }
    Ok(documents)
}

fn write_artifacts(request: &Request, dir: &str, artifacts: BTreeMap<CellId, Vec<u8>>) -> Result<(), String> {
    std::fs::create_dir_all(request.output.join(dir)).map_err(|e| e.to_string())?;
    for (cell, bytes) in artifacts {
        let width = obc_map_core::grid::id_width(cell.log2);
        let path = request.output.join(format!("{dir}/{:0width$}/{:0width$}.bin", cell.i, cell.j));
        std::fs::create_dir_all(path.parent().expect("an artifact path has a parent"))
            .map_err(|e| format!("{}: {e}", path.display()))?;
        std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(())
}

/// Each file of `files` in the new directory `dir`, as a hard link or else a copy: the compiler
/// refuses a symbolic link, which could name a file outside its directory.
fn copied_view(files: &BTreeMap<String, PathBuf>, dir: &Path) -> Result<(), String> {
    for (path, object) in files {
        if path.split('/').any(|part| part.is_empty() || part == "." || part == "..") {
            return Err(format!("{path} is not a relative path"));
        }
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().expect("a joined path has a parent"))
            .map_err(|e| format!("{}: {e}", file.display()))?;
        std::fs::hard_link(object, &file)
            .or_else(|_| std::fs::copy(object, &file).map(drop))
            .map_err(|e| format!("{}: {e}", file.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::landmarks::peaks::{discover, PeakContent};

    #[test]
    fn area_content_with_the_same_relative_path_preserves_unique_and_overlapping_articles() {
        use obc_formats::io::SliceSource;
        use obc_reader::peaks::Directory;
        let dir = obcm_testkit::scratch::scratch_dir("step", "area-content");
        let article = |id: &str, node: u64| {
            serde_json::json!({
                "schema":1, "collection":"peaks", "input_sha256":"input", "policy_sha256":"policy",
                "languages":["en","de","fr","es"], "source_coverage":{},
                "counts":crate::landmarks::Counts::default(), "omissions":[],
                "records":[{"id":id, "name":id, "default_language":"en", "fallback_sources":[], "photo":null,
                    "variants":[{"language":"en", "text_pages":[id], "attribution":{
                        "source_url":"https://en.wikipedia.org/w/index.php?title=Mountain&oldid=1",
                        "revision":"1", "license_url":"https://creativecommons.org/licenses/by-sa/4.0/",
                        "original_notices":"Authors"}}]}],
                "associations":[{"node_id":node, "article_id":id, "latitude":0, "longitude":0}]
            })
        };
        let mut east = article("Q1", 1);
        let other = article("Q2", 2);
        east["records"].as_array_mut().unwrap().push(other["records"][0].clone());
        east["associations"].as_array_mut().unwrap().push(other["associations"][0].clone());
        let mut layers = BTreeMap::new();
        for (area, content) in [("west", article("Q1", 1)), ("east", east)] {
            let file = dir.join(format!("{area}.json"));
            std::fs::write(&file, content.to_string()).unwrap();
            layers.insert(format!("maps/peak-content/{area}"), BTreeMap::from([("peaks/peaks.json".into(), file)]));
        }
        let cell = CellId::containing(18, 0, 0);
        let request = Request {
            step: "maps/peaks/leaf".into(),
            snapshots: BTreeMap::new(),
            layers,
            layer_files: BTreeMap::new(),
            options: serde_json::json!({"cell_log2":18,"cells":[[cell.i,cell.j]]}),
            output: dir.join("output"),
            metrics: dir.join("metrics"),
        };
        peaks(&request).unwrap();
        let bytes = std::fs::read(request.output.join(format!("peaks/{:04}/{:04}.bin", cell.i, cell.j))).unwrap();
        let reader = Directory::read(&SliceSource(&bytes)).unwrap();
        assert_eq!((reader.associations, reader.records), (2, 2));
        let mut empty = article("Q1", 1);
        empty["records"] = serde_json::json!([]);
        empty["associations"] = serde_json::json!([]);
        let file = dir.join("empty.json");
        std::fs::write(&file, empty.to_string()).unwrap();
        let empty = Request {
            layers: BTreeMap::from([(
                "maps/peak-content/empty".into(),
                BTreeMap::from([("peaks/peaks.json".into(), file)]),
            )]),
            output: dir.join("empty").join("output"),
            ..request
        };
        peaks(&empty).unwrap();
        assert!(empty.output.join("peaks").is_dir(), "valid empty content fulfills the declared output");
        assert_eq!(std::fs::read_dir(empty.output.join("peaks")).unwrap().count(), 0, "no placeholder payload");
    }

    /// The capture keeps no summits, which are OSM data: the step makes them again from the
    /// extract, byte for byte, and refuses a capture whose recipe pinned others, as after a change
    /// of the code that makes them.
    #[test]
    fn a_peak_capture_compiles_with_the_summits_that_the_step_makes_again() {
        let dir = obcm_testkit::scratch::scratch_dir("step", "peak-content");
        let osm = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/peak-discovery.osm.pbf");
        let poly = dir.join("area.poly");
        std::fs::write(&poly, "world\n1\n -179 -89\n 179 -89\n 179 89\n -179 89\n -179 -89\nEND\nEND\n").unwrap();
        let capture = dir.join("capture");
        std::fs::create_dir_all(&capture).unwrap();
        let boundary = capture.join("boundary.geojson");
        std::fs::write(&boundary, crate::catalog::boundary::geojson(&std::fs::read_to_string(&poly).unwrap()).unwrap())
            .unwrap();
        let summits = capture.join("summits.json");
        discover(&osm, &boundary, &summits).unwrap();
        let hex = |bytes: &[u8]| Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect::<String>();
        let bytes = std::fs::read(&summits).unwrap();
        let manifest = capture.join("manifest.json");
        let source = serde_json::json!({"path": "summits.json", "url": "urn:summits", "bytes": bytes.len(), "sha256": hex(&bytes)});
        let document = serde_json::json!({"schema": 1, "sources": [source], "places": [],
            "peaks": {"summits_path": "summits.json", "resolutions": []}});
        std::fs::write(&manifest, serde_json::to_vec(&document).unwrap()).unwrap();

        let files = |name: &str, path: &Path| (name.to_string(), path.to_path_buf());
        let boundary_sha256 = hex(&std::fs::read(&boundary).unwrap());
        let build = |name: &str, boundary_sha256: &str, summits_sha256: &str| {
            let recipe = dir.join(format!("{name}.json"));
            let pins = serde_json::json!({"boundary_sha256": boundary_sha256, "summits_sha256": summits_sha256});
            std::fs::write(&recipe, pins.to_string()).unwrap();
            let capture = [files("#peaks=0/manifest.json", &manifest), files("#peaks=0/recipe.json", &recipe)];
            let request = Request {
                step: "maps/peak-content".into(),
                snapshots: BTreeMap::from([
                    ("wikidata".to_string(), BTreeMap::from(capture)),
                    ("geofabrik-poly".to_string(), BTreeMap::from([files("area.poly", &poly)])),
                    ("geofabrik-extracts".to_string(), BTreeMap::from([files("area.osm.pbf", &osm)])),
                ]),
                layers: BTreeMap::new(),
                layer_files: BTreeMap::new(),
                options: serde_json::json!({}),
                output: dir.join(name).join("output"),
                metrics: dir.join(name).join("metrics.json"),
            };
            peak_content(&request).map(|()| request.output)
        };
        let output = build("current", &boundary_sha256, &hex(&bytes)).unwrap();
        let content = std::fs::read(output.join("peaks/peaks.json")).unwrap();
        let content: PeakContent = serde_json::from_slice(&content).unwrap();
        let summits: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(content.counts.captured > 0);
        assert_eq!(content.counts.captured, summits["summits"].as_array().unwrap().len());
        for (name, boundary_pin, summit_pin, changed) in [
            ("other-summits", boundary_sha256.as_str(), hex(b"other summits"), "summits"),
            ("other-boundary", "other boundary", hex(&bytes), "boundary"),
        ] {
            let refused = build(name, boundary_pin, &summit_pin).unwrap_err();
            assert!(refused.contains(&format!("the {changed} that it pinned")), "{refused}");
            assert!(refused.ends_with("Plan with `--move wikidata`"), "{refused}");
            assert!(!dir.join(name).join("output/peaks/peaks.json").exists(), "pin mismatch writes no content");
        }
    }
}
