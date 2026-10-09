use super::*;
use crate::store::tests::Scratch;

fn prepared(scratch: &Scratch, store: &Store, label: &str) -> Selected {
    let out = scratch.0.join(label);
    fs::create_dir_all(&out).unwrap();
    let index = out.join("index.sqlite");
    fs::write(&index, format!("{label}-lookup")).unwrap();
    let index_hash = store::hash_file(&index).unwrap().0;
    let bundle = serde_json::to_vec(&json!({"schema":1,"records":[],"label":label})).unwrap();
    let bundle_hash = store::sha256_hex(&bundle);
    fs::write(out.join(&bundle_hash), &bundle).unwrap();
    let manifest = json!({"schema":1,"complete":true,"records":0,"coverage":{},"origins":[],"files":[
        {"name":"index.sqlite","sha256":index_hash,"bytes":fs::metadata(&index).unwrap().len(),"kind":"index"},
        {"name":bundle_hash,"sha256":bundle_hash,"bytes":bundle.len(),"kind":"bundle"}]});
    fs::write(out.join("manifest.json"), serde_json::to_vec(&manifest).unwrap()).unwrap();
    admit(store, &out, &manifest).unwrap()
}

#[test]
fn publication_announces_only_verified_complete_content_and_restore_keeps_the_pin() {
    let scratch = Scratch::new("content-publication");
    let store = Store::at(scratch.0.join("store"));
    let selected = prepared(&scratch, &store, "first");
    let bucket = Bucket::local(&scratch.0.join("bucket"));
    let published = publish(&store, &bucket, &selected.sha256).unwrap();
    assert_eq!(published["objects"], 3);
    assert_eq!(publish(&store, &bucket, &selected.sha256).unwrap(), published);
    let fresh = Store::at(scratch.0.join("fresh"));
    restore(&fresh, &bucket, &selected.sha256).unwrap();
    assert_eq!(super::selected(&fresh).unwrap(), Some(selected.clone()));
    assert_eq!(manifest(&fresh, &selected.sha256).unwrap(), manifest(&store, &selected.sha256).unwrap());
    let snapshot = fresh.snapshot(SOURCE, &selected.sha256).unwrap().unwrap();
    assert_eq!(
        snapshot.files.iter().filter(|file| fresh.object(&file.sha256).is_file()).count(),
        2,
        "restore fetches the manifest and index, not every content bundle"
    );
    let second = prepared(&scratch, &store, "second");
    let value = manifest(&store, &second.sha256).unwrap();
    let bundle = value["files"][1]["sha256"].as_str().unwrap();
    let path = store.object(bundle);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    fs::write(path, b"corrupt").unwrap();
    assert!(publish(&store, &bucket, &second.sha256).unwrap_err().contains("changed"));
    assert!(bucket.read(&format!("content/manifests/{}.json", second.sha256)).unwrap().is_none());
    assert!(bucket.read(&format!("content/manifests/{}.json", selected.sha256)).unwrap().is_some());
}

#[test]
fn the_selected_older_snapshot_stays_reachable_and_incomplete_manifests_are_refused() {
    let scratch = Scratch::new("content-roots");
    let store = Store::at(scratch.0.join("store"));
    let old = prepared(&scratch, &store, "old");
    use_local(&store, &old.sha256).unwrap();
    prepared(&scratch, &store, "new");
    let gc = crate::store::gc::plan(&store, &Default::default()).unwrap();
    assert!(!gc.snapshots.contains(&format!("{SOURCE}@{}", old.sha256)));
    assert!(!gc.objects.iter().any(|(digest, _)| digest == &old.sha256));
    let mut invalid = manifest(&store, &old.sha256).unwrap();
    invalid["complete"] = json!(false);
    assert!(check_manifest(&invalid).is_err());
    invalid["complete"] = json!(true);
    invalid["files"][1]["bytes"] = json!(16 * 1024 * 1024 + 1);
    assert!(check_manifest(&invalid).is_err());
    let mut incompatible = manifest(&store, &old.sha256).unwrap();
    incompatible["files"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"a".repeat(64),"sha256":"a".repeat(64),"bytes":216*240,"kind":"image"}));
    incompatible["photo_transform_sha256"] = json!("b".repeat(64));
    let file = store.partial("incompatible-photo-manifest");
    let bytes = serde_json::to_vec(&incompatible).unwrap();
    store::write_atomic(&file, &bytes).unwrap();
    let digest = store::sha256_hex(&bytes);
    store.insert(&file, &digest).unwrap();
    let photo = scratch.0.join("host/obc-pack/src/landmarks/photo.rs");
    fs::create_dir_all(photo.parent().unwrap()).unwrap();
    fs::write(photo, b"current transform").unwrap();
    assert!(resolve(&scratch.0, &store, &scratch.0.join("work"), &json!({"snapshot":digest}))
        .unwrap_err()
        .contains("photo transform changed"));
    assert_eq!(super::selected(&store).unwrap(), Some(old));
}

#[test]
fn configuration_uses_absolute_inputs_without_reading_or_fetching_archives() {
    let scratch = Scratch::new("content-config");
    let store = Store::at(scratch.0.join("store"));
    let path = scratch.0.join("config.json");
    fs::write(&path,serde_json::to_vec(&json!({"wikidata":{"path":"all.json.bz2","date":"2026-01-01"},"wikipedia":{"en":{"path":"en.tar.gz","date":"2026-01-01"}},"osm":["planet.osm.pbf"]})).unwrap()).unwrap();
    let request = configure(&store, &path).unwrap();
    assert_eq!(request.config["wikidata"]["path"], json!(scratch.0.join("all.json.bz2")));
    assert_eq!(request.config["osm"][0], json!(scratch.0.join("planet.osm.pbf")));
    assert_eq!(configured(&store).unwrap(), request);
    assert!(selected(&store).unwrap().is_none());
}
