//! Saved package releases keep their exact build inputs through store cleanup.

use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Saved {
    pub package: String,
    pub bootstrap: LayerFile,
    pub inputs: Inputs,
    pub release: Release,
    pub archive: LayerFile,
}

impl Saved {
    pub fn check(&self) -> Result<(), String> {
        if !crate::is_kebab(&self.package) || self.release.product != "maps" {
            return Err("saved fixture package has no known map release".into());
        }
        digest(&self.bootstrap.sha256)?;
        digest(&self.archive.sha256)?;
        if self.archive.path != format!("packages/{}.tar.gz", self.archive.sha256) {
            return Err("saved fixture archive is not content addressed".into());
        }
        self.release.check_named()?;
        for layer in &self.release.layers {
            for file in &layer.files {
                relative(&file.path)?;
                digest(&file.sha256)?;
            }
            for (source, read) in &layer.snapshots {
                if !crate::is_kebab(source) || read.version.is_empty() || read.version.contains('/') {
                    return Err("saved fixture has an invalid source version".into());
                }
            }
        }
        Ok(())
    }

    pub fn write(&self, store: &Store) -> Result<(), String> {
        self.check()?;
        crate::commit::durable(
            &store.root().join("fixtures").join(format!("{}.json", self.package)),
            &serde_json::to_vec(self).map_err(|e| e.to_string())?,
        )
    }
}

/// Corrupt saved selections block cleanup instead of losing their input roots.
pub fn saved(store: &Store) -> Result<Vec<Saved>, String> {
    let entries = match std::fs::read_dir(store.root().join("fixtures")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let mut saved = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.path().extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        if !entry.file_type().map_err(|e| e.to_string())?.is_file() {
            return Err("saved fixture record must be a regular file".into());
        }
        let record: Saved = serde_json::from_slice(&std::fs::read(entry.path()).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        record.check()?;
        if entry.file_name().to_str() != Some(format!("{}.json", record.package).as_str()) {
            return Err("saved fixture record belongs to another package".into());
        }
        saved.push(record);
    }
    saved.sort_by(|left, right| left.package.cmp(&right.package));
    Ok(saved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::release::{Layer, SnapshotRead};
    use crate::engine::Client;
    use crate::store::{gc, FileRecord, Snapshot};

    #[test]
    fn saved_packages_keep_old_source_versions_and_corrupt_roots_refuse_cleanup() {
        let scratch = crate::store::tests::Scratch::new("fixture-roots");
        let store = Store::at(&scratch.0);
        let mut files = Vec::new();
        for (version, bytes, retrieved) in [("1", b"old".as_slice(), "2026-01-01"), ("2", b"new", "2026-02-01")] {
            let sha256 = sha256_hex(bytes);
            crate::store::write_atomic(&store.partial("input"), bytes).unwrap();
            store.insert(&store.partial("input"), &sha256).unwrap();
            store
                .put_snapshot(&Snapshot {
                    source: "fixture-osm".into(),
                    version: version.into(),
                    files: vec![FileRecord {
                        name: "crop.osm.pbf".into(),
                        url: format!("https://fixtures.example/{sha256}"),
                        size: bytes.len() as u64,
                        sha256: sha256.clone(),
                        retrieved: retrieved.into(),
                    }],
                })
                .unwrap();
            files.push(LayerFile { path: "crop.osm.pbf".into(), size: bytes.len() as u64, sha256 });
        }
        let layer = Layer {
            step: "maps/captured".into(),
            key: "1".repeat(64),
            inputs: Vec::new(),
            options: serde_json::json!({}),
            code: "2".repeat(64),
            command: None,
            outputs: vec!["crop.osm.pbf".into()],
            digest: crate::engine::layer_digest(&files[..1], &[]),
            files: files[..1].to_vec(),
            snapshots: [("fixture-osm".into(), SnapshotRead { version: "1".into(), params: Vec::new() })].into(),
            client: Client::None,
        };
        let selected = Saved {
            package: "ride".into(),
            bootstrap: files[0].clone(),
            inputs: Inputs {
                osm: CapturedInput { source: "fixture-osm".into(), version: "1".into(), files: Vec::new() },
                osm_sha256: files[0].sha256.clone(),
                content: BTreeMap::new(),
            },
            release: Release::compose("maps", "ride", &[], None, vec![layer], &Default::default()),
            archive: LayerFile { path: format!("packages/{}.tar.gz", files[0].sha256), ..files[0].clone() },
        };
        selected.write(&store).unwrap();
        let plan = gc::plan(&store, &gc::Roots::default()).unwrap();
        assert!(!plan.snapshots.contains(&"fixture-osm@1".into()));
        assert!(plan.kept.iter().any(|kept| kept.entry == "fixture-osm@1" && kept.because == ["fixture ride"]));
        assert!(!plan.objects.iter().any(|(hash, _)| hash == &files[0].sha256));
        std::fs::write(store.root().join("fixtures/ride.json"), b"invalid record").unwrap();
        assert!(gc::plan(&store, &gc::Roots::default()).is_err());
    }
}
