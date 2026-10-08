//! Exact identity requests and regional compilation dependencies.

use super::*;
use serde_json::Value;

pub(super) fn captures(
    env: &Env,
    store: &Store,
    collection: &'static str,
    osm: &str,
    poly: &str,
    extract: &str,
    area: &[(String, String)],
) -> Result<Vec<(&'static str, Vec<Input>)>, Unplanned> {
    let fail = Unplanned::Failed;
    let osm_path = store.object(osm.strip_prefix("sha256:").ok_or_else(|| fail("missing OSM digest".into()))?);
    let mut qids = Vec::new();
    let mut links = Vec::new();
    if collection == "landmarks" {
        qids = obc_pack::landmarks::discover::candidates(&osm_path, &osm[7..]).map_err(fail)?.qids;
    } else {
        let scratch = store.partial(&format!("content-discovery-{}", &osm[7..23]));
        std::fs::create_dir_all(&scratch).map_err(|e| fail(e.to_string()))?;
        let boundary = scratch.join("boundary.json");
        let polygon = std::fs::read_to_string(store.object(&poly[7..])).map_err(|e| fail(e.to_string()))?;
        std::fs::write(&boundary, obc_pack::catalog::boundary::geojson(&polygon).map_err(fail)?)
            .map_err(|e| fail(e.to_string()))?;
        let path = scratch.join("summits.json");
        obc_pack::landmarks::peaks::discover(&osm_path, &boundary, &path).map_err(fail)?;
        let source: obc_pack::landmarks::peaks::SummitSource =
            serde_json::from_slice(&std::fs::read(&path).map_err(|e| fail(e.to_string()))?)
                .map_err(|e| fail(e.to_string()))?;
        for summit in source.summits {
            for tag in ["wikidata", "wikipedia"] {
                if let Some(value) = summit.tags.get(tag) {
                    links.push(format!("{tag}:{value}"));
                }
            }
        }
        links.sort();
        links.dedup();
    }
    let mut query =
        serde_json::json!({"entities":qids,"links":links,"articles":[],"commons":[],"files":[],"categories":[]});
    let mut inputs = Vec::new();
    let mut phase = 0;
    loop {
        let params = vec![("content".into(), query.to_string())];
        inputs.clear();
        let mut missing = Vec::new();
        let mut files = BTreeMap::new();
        for source in CAPTURES {
            if let Some(version) = snapshot_version(env, store, source, &params, &mut missing)? {
                if let Some(available) = snapshot_files(store, source, &version, &params, &[]).map_err(fail)? {
                    for (name, object) in available {
                        files.insert(name.trim_start_matches('#').to_owned(), object);
                    }
                    inputs.push(Input::Snapshot {
                        source: source.into(),
                        version,
                        params: params.clone(),
                        files: Vec::new(),
                    });
                } else {
                    missing.push(Wanted { source: source.into(), version: Some(version), params: params.clone() });
                }
            }
        }
        if !missing.is_empty() {
            return Err(Unplanned::NeedsFetch(missing));
        }
        let facts = obc_pack::landmarks::shared::facts(&files).map_err(fail)?;
        if phase == 0 {
            let subjects = if collection == "landmarks" {
                obc_pack::landmarks::shared::selected(&facts, &qids)
            } else {
                facts
                    .values()
                    .filter(|fact| fact["kind"] == "link" && fact["status"] == "present")
                    .filter_map(|fact| fact["identity"].as_str().map(str::to_owned))
                    .collect()
            };
            let mut articles = Vec::new();
            query["subjects"] = serde_json::json!(subjects);
            for id in subjects {
                if let Some(entity) = obc_pack::landmarks::shared::subject(&facts, &id) {
                    for (wiki, link) in entity["sitelinks"].as_object().into_iter().flatten() {
                        if let Some(language) = wiki.strip_suffix("wiki") {
                            articles.push(serde_json::json!({"language":language,"title":link["title"],"qid":entity["id"].as_str().filter(|id|id.starts_with('Q'))}));
                        }
                    }
                }
            }
            query["articles"] = serde_json::json!(articles);
            let mut categories = BTreeSet::new();
            for fact in facts.values().filter(|fact| {
                fact["kind"] == "entity"
                    && query["subjects"].as_array().is_some_and(|subjects| subjects.contains(&fact["key"]))
            }) {
                categories.extend(obc_pack::landmarks::shared::categories(&fact["entity"]));
            }
            query["categories"] = serde_json::json!(categories);
        } else if phase == 1 {
            let mut commons = BTreeSet::new();
            for fact in facts.values().filter(|fact| {
                fact["kind"] == "entity"
                    && query["subjects"].as_array().is_some_and(|subjects| subjects.contains(&fact["key"]))
            }) {
                for claim in fact["entity"]["claims"]["P18"].as_array().into_iter().flatten() {
                    if let Some(file) = claim["mainsnak"]["datavalue"]["value"].as_str() {
                        commons.insert(file.to_owned());
                    }
                }
            }
            for fact in facts.values().filter(|fact| fact["kind"] == "article" && fact["status"] == "present") {
                if let Some(filename) = obc_pack::landmarks::shared::lead_image(fact) {
                    commons.insert(filename);
                }
            }
            for fact in facts.values().filter(|fact| fact["kind"] == "category" && fact["status"] == "present") {
                commons.extend(
                    fact["members"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|member| member["title"].as_str())
                        .filter_map(|name| name.strip_prefix("File:"))
                        .map(str::to_owned),
                );
            }
            query["commons"] = serde_json::json!(commons);
        } else {
            let scratch = store.partial(&format!(
                "content-selection-{}",
                &obc_data::store::sha256_hex(query.to_string().as_bytes())[..32]
            ));
            let view = scratch.join("view");
            std::fs::create_dir_all(&view).map_err(|e| fail(e.to_string()))?;
            let boundary = scratch.join("boundary.json");
            let polygon = std::fs::read_to_string(store.object(&poly[7..])).map_err(|e| fail(e.to_string()))?;
            std::fs::write(&boundary, obc_pack::catalog::boundary::geojson(&polygon).map_err(fail)?)
                .map_err(|e| fail(e.to_string()))?;
            let ids = query["subjects"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>();
            let output = scratch.join("selection");
            if output.exists() {
                std::fs::remove_dir_all(&output).map_err(|e| fail(e.to_string()))?;
            }
            let requested = if collection == "landmarks" {
                obc_pack::landmarks::shared::view(&files, &view, &ids).map_err(fail)?;
                obc_pack::landmarks::photo_requests(&view.join("manifest.json"), &boundary, &output, None)
                    .map_err(fail)?
                    .requests
            } else {
                obc_pack::landmarks::peaks::discover(&osm_path, &boundary, &view.join("summits.json")).map_err(fail)?;
                obc_pack::landmarks::shared::peak_view(&files, &view).map_err(fail)?;
                obc_pack::landmarks::peaks::compile(&view.join("manifest.json"), &boundary, &output, true)
                    .map_err(fail)?
                    .photo_requests
            };
            if requested.is_empty() {
                break;
            }
            let mut wanted = query["files"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<BTreeSet<_>>();
            wanted.extend(requested.into_iter().map(|request| request.filename));
            let next = serde_json::json!(wanted);
            if next == query["files"] {
                return Err(fail("selected media input remains unresolved".into()));
            }
            query["files"] = next;
        }
        phase += 1;
    }
    inputs.extend([
        Input::Snapshot { source: EXTRACTS.into(), version: extract.into(), params: area.to_vec(), files: Vec::new() },
        Input::Snapshot {
            source: POLY.into(),
            version: version(env, store, POLY, area)
                .map_err(fail)?
                .map_err(|fetch| Unplanned::NeedsFetch(vec![fetch]))?,
            params: area.to_vec(),
            files: Vec::new(),
        },
    ]);
    Ok(vec![(collection, inputs)])
}
