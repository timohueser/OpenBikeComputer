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
    let mut subjects = BTreeSet::new();
    loop {
        if qids.is_empty() && links.is_empty() {
            break;
        }
        let params = vec![("content".into(), query.to_string())];
        if let Some((_, error)) = env
            .fetch_failures
            .iter()
            .find(|(request, _)| CAPTURES.contains(&request.source.as_str()) && request.params == params)
        {
            return Err(Unplanned::Invalid(format!("Wikimedia acquisition failed: {error}; prepare again to retry")));
        }
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
            subjects = if collection == "landmarks" {
                obc_pack::landmarks::shared::selected(&facts, &qids)
            } else {
                facts
                    .values()
                    .filter(|fact| fact["kind"] == "link" && fact["status"] == "present")
                    .filter_map(|fact| fact["identity"].as_str().map(str::to_owned))
                    .collect()
            };
            let mut articles = Vec::new();
            for id in &subjects {
                if let Some(entity) = obc_pack::landmarks::shared::subject(&facts, id) {
                    for (wiki, link) in entity["sitelinks"].as_object().into_iter().flatten() {
                        if let Some(language) = wiki.strip_suffix("wiki") {
                            articles.push(serde_json::json!({"language":language,"title":link["title"]}));
                        }
                    }
                }
            }
            query["articles"] = serde_json::json!(articles);
            let mut categories = BTreeSet::new();
            for fact in facts.values().filter(|fact| {
                fact["kind"] == "entity" && fact["key"].as_str().is_some_and(|key| subjects.contains(key))
            }) {
                categories.extend(obc_pack::landmarks::shared::categories(&fact["entity"]));
            }
            query["categories"] = serde_json::json!(categories);
        } else if phase == 1 {
            let mut commons = BTreeSet::new();
            for fact in facts.values().filter(|fact| {
                fact["kind"] == "entity" && fact["key"].as_str().is_some_and(|key| subjects.contains(key))
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
            if view.exists() {
                std::fs::remove_dir_all(&view).map_err(|e| fail(e.to_string()))?;
            }
            std::fs::create_dir_all(&view).map_err(|e| fail(e.to_string()))?;
            let boundary = scratch.join("boundary.json");
            let polygon = std::fs::read_to_string(store.object(&poly[7..])).map_err(|e| fail(e.to_string()))?;
            std::fs::write(&boundary, obc_pack::catalog::boundary::geojson(&polygon).map_err(fail)?)
                .map_err(|e| fail(e.to_string()))?;
            let ids = subjects.iter().cloned().collect::<Vec<_>>();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapping_regions_request_the_same_identities_and_failures_stay_visible() {
        let scratch = super::super::tests::temp("shared-content-identities");
        let store = Store::at(scratch.0.join("store"));
        let pbf =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../obc-pack/tests/data/peak-discovery.osm.pbf");
        let bytes = std::fs::read(pbf).unwrap();
        let digest = obc_data::store::sha256_hex(&bytes);
        let file = store.partial("osm");
        obc_data::store::write_atomic(&file, &bytes).unwrap();
        store.insert(&file, &digest).unwrap();
        let osm = format!("sha256:{digest}");
        let wanted = |env: &Env, area: &str| match captures(
            env,
            &store,
            "landmarks",
            &osm,
            &format!("sha256:{}", "0".repeat(64)),
            "1",
            &[("area".into(), area.into())],
        ) {
            Err(Unplanned::NeedsFetch(wanted)) => wanted,
            _ => panic!("unprepared identities need source facts"),
        };
        let mut env = Env::default();
        let first = wanted(&env, "one");
        let second = wanted(&env, "overlap");
        assert_eq!(first, second);
        assert_eq!(first.len(), 3);
        let query: Value = serde_json::from_str(&first[0].params[0].1).unwrap();
        assert_eq!(query["entities"], serde_json::json!(["Q1"]));
        assert!(query.get("area").is_none());
        assert!(query.get("code").is_none());
        env.fetch_failures.push((first[0].clone(), "server pressure".into()));
        let failed = captures(
            &env,
            &store,
            "landmarks",
            &osm,
            &format!("sha256:{}", "0".repeat(64)),
            "1",
            &[("area".into(), "one".into())],
        );
        assert!(matches!(failed,Err(Unplanned::Invalid(reason)) if reason.contains("server pressure")));
    }
}
