//! Saved package releases keep their exact build inputs through store cleanup.

use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub package: String,
    /// The source coverage of the selected PBF, longitude first.
    pub coverage: [f64; 4],
    pub inputs: Inputs,
    pub release: Release,
    pub glo30: String,
    pub copies: Vec<crate::input_copy::Retained>,
    pub assets: BTreeMap<String, LayerFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Saved {
    pub selection: Selection,
    pub archive: LayerFile,
}

impl Selection {
    pub fn new(
        store: &Store,
        package: String,
        coverage: [f64; 4],
        mut inputs: Inputs,
        release: Release,
        env: &Env,
        assets: BTreeMap<String, LayerFile>,
    ) -> Result<Self, String> {
        let mut reads = crate::input_copy::reads_release(&release)?;
        if let Some(terrain) = &mut inputs.terrain {
            let selected: std::collections::BTreeSet<_> = reads
                .iter()
                .filter(|read| {
                    read.key.source == terrain.source
                        && read.key.version == terrain.version
                        && read.params == terrain.params
                })
                .flat_map(|read| &read.files)
                .collect();
            terrain.files.retain(|file| selected.contains(file));
            if terrain.files.is_empty() {
                inputs.terrain = None;
            }
        }
        for (destination, input) in &inputs.historical {
            let files =
                crate::engine::snapshot_files(store, &input.source, &input.version, &input.params, &input.files)?
                    .ok_or("historical fixture input is missing")?;
            let hashes = files
                .iter()
                .map(|(name, path)| hash_file(path).map(|(hash, _)| (name.clone(), hash)))
                .collect::<Result<BTreeMap<_, _>, _>>()?;
            reads.push(crate::input_copy::Read {
                step: format!("historical/{destination}"),
                key: crate::input_copy::Key {
                    source: input.source.clone(),
                    version: input.version.clone(),
                    digest: crate::engine::digest(hashes.iter().map(|(name, hash)| (name.as_str(), hash.as_str()))),
                },
                params: input.params.clone(),
                files: hashes.into_keys().collect(),
            });
        }
        let mut copies = BTreeMap::new();
        for read in reads {
            let record = crate::input_copy::Record::local(store, &read)?
                .ok_or("fixture selection lacks an exact input copy record")?;
            copies.entry((read.key.clone(), read.params.clone())).or_insert(crate::input_copy::Retained {
                step: read.step,
                key: read.key,
                params: read.params,
                record,
            });
        }
        let glo30 = env
            .version("copernicus-glo-30", &[])?
            .ok_or("fixture selection lacks its terrain source version")?
            .to_string();
        let selected =
            Self { package, coverage, inputs, release, glo30, copies: copies.into_values().collect(), assets };
        selected.check()?;
        Ok(selected)
    }

    pub fn local(store: &Store, package: &str) -> Result<Option<Saved>, String> {
        relative(package)?;
        match std::fs::read(store.root().join("fixtures").join(format!("{package}.json"))) {
            Ok(bytes) => {
                let saved: Saved = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                saved.check()?;
                if saved.selection.package != package {
                    return Err("fixture selection belongs to another package".into());
                }
                Ok(Some(saved))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    pub(crate) fn basis(store: &Store, catalog: &Catalog, package: &str) -> Result<Option<Saved>, String> {
        let saved = Self::local(store, package)?;
        if let Some(published) = catalog.selection(package)? {
            if saved.as_ref().is_none_or(|saved| saved.archive != published) {
                return Err(
                    "published fixture selection requires explicit preparation; plan does not download its archive"
                        .into(),
                );
            }
        }
        Ok(saved)
    }

    pub(crate) fn recover(
        root: &Path,
        store: &Store,
        http: &crate::fetch::http::Http,
        catalog: &Catalog,
        package: &str,
        archive: LayerFile,
        moves: &BTreeMap<String, Option<String>>,
    ) -> Result<Saved, String> {
        let url = format!("{}{}", catalog.base_url, archive.path);
        let _lock = crate::fetch::http::Http::lock(store, &url)?;
        let object = http.download(
            store,
            &url,
            &crate::fetch::http::Expect { sha256: Some(&archive.sha256), ..Default::default() },
        )?;
        if hash_file(&object.object)? != (archive.sha256.clone(), archive.size) {
            return Err("fixture archive differs from its catalog".into());
        }
        let scratch = crate::r2::Scratch::new()?;
        let tree = scratch.0.join("selection");
        materialize(
            root,
            &[
                "import".into(),
                package.into(),
                object.object.to_string_lossy().into(),
                archive.sha256.clone(),
                tree.to_string_lossy().into(),
            ],
            None,
        )?;
        let selection: Self =
            serde_json::from_slice(&std::fs::read(tree.join(".obc-data.json")).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        selection.check()?;
        if selection.package != package {
            return Err("fixture archive contains another selection".into());
        }
        for (destination, asset) in &selection.assets {
            let path = tree.join(destination);
            if hash_file(&path)? != (asset.sha256.clone(), asset.size) {
                return Err("fixture asset differs from its selection".into());
            }
            let part = store.partial(&format!("fixture-{}", asset.sha256));
            std::fs::copy(path, &part).map_err(|e| e.to_string())?;
            store.insert(&part, &asset.sha256)?;
        }
        selection.restore(
            store,
            http,
            &crate::live::Remote::Public(catalog.base_url.trim_end_matches('/').into()),
            moves,
        )?;
        let saved = Saved { selection, archive };
        saved.write(store)?;
        Ok(saved)
    }

    pub fn identity(&self) -> Result<String, String> {
        self.check()?;
        Ok(sha256_hex(&serde_json::to_vec(self).map_err(|e| e.to_string())?))
    }

    pub fn environment(&self, region: &str) -> Env {
        let mut env = Env { name: format!("fixtures-{}", self.package), region: region.into(), ..Default::default() };
        env.live.insert(("copernicus-glo-30".into(), Vec::new()), [self.glo30.clone()].into());
        for copy in &self.copies {
            env.live
                .entry((copy.key.source.clone(), copy.params.clone()))
                .or_default()
                .insert(copy.key.version.clone());
        }
        env.retained = self.copies.clone();
        env
    }

    pub fn restore(
        &self,
        store: &Store,
        http: &crate::fetch::http::Http,
        remote: &crate::live::Remote,
        moves: &BTreeMap<String, Option<String>>,
    ) -> Result<(), String> {
        self.check()?;
        for copy in &self.copies {
            if moves.contains_key(&copy.key.source)
                && !self.inputs.historical.values().any(|input| input.source == copy.key.source)
            {
                continue;
            }
            let selected = copy.record.files.iter().map(|file| file.name.clone()).collect::<Vec<_>>();
            copy.record.materialize(&copy.key, store, http, remote, &copy.params, &selected)?;
        }
        Ok(())
    }

    pub fn check(&self) -> Result<(), String> {
        if !crate::is_kebab(&self.package) || self.release.product != "maps" {
            return Err("saved fixture package has no known map release".into());
        }
        crate::regions::Bbox::new(self.coverage)?;
        self.inputs.check()?;
        for (destination, asset) in &self.assets {
            relative(destination)?;
            relative(&asset.path)?;
            digest(&asset.sha256)?;
        }
        self.release.check_named()?;
        let mut copies = std::collections::BTreeSet::new();
        for copy in &self.copies {
            copy.record.validate(&copy.key)?;
            if crate::store::sorted(&copy.params) != copy.params
                || !copies.insert((&copy.key.source, &copy.key.version, &copy.params))
            {
                return Err("fixture input copies have duplicate or non-canonical requests".into());
            }
        }
        for read in crate::input_copy::reads_release(&self.release)? {
            let copy = self
                .copies
                .iter()
                .find(|copy| copy.key == read.key && copy.params == read.params)
                .ok_or("fixture selection lacks a release input copy")?;
            if copy.record.files.iter().map(|file| &file.name).collect::<Vec<_>>()
                != read.files.iter().collect::<Vec<_>>()
            {
                return Err("fixture copy files differ from their release read".into());
            }
        }
        if self.glo30.is_empty() || self.glo30.contains(['/', '\\']) || self.glo30.chars().any(char::is_control) {
            return Err("fixture selection has no normalized terrain source version".into());
        }
        for input in std::iter::once(&self.inputs.osm)
            .chain(self.inputs.content.values())
            .chain(&self.inputs.terrain)
            .chain(self.inputs.historical.values())
        {
            let files: BTreeMap<_, _> = self
                .copies
                .iter()
                .filter(|copy| {
                    copy.key.source == input.source && copy.key.version == input.version && copy.params == input.params
                })
                .flat_map(|copy| &copy.record.files)
                .map(|file| (&file.name, file))
                .collect();
            if files.is_empty() || input.files.iter().any(|name| !files.contains_key(name)) {
                return Err("fixture selected input has no exact retained files".into());
            }
        }
        if !self
            .copies
            .iter()
            .filter(|copy| {
                copy.key.source == self.inputs.osm.source
                    && copy.key.version == self.inputs.osm.version
                    && copy.params == self.inputs.osm.params
            })
            .flat_map(|copy| &copy.record.files)
            .any(|file| self.inputs.osm.files.contains(&file.name) && file.sha256 == self.inputs.osm_sha256)
        {
            return Err("fixture PBF hash differs from its retained input".into());
        }
        for layer in &self.release.layers {
            for file in &layer.files {
                relative(&file.path)?;
                digest(&file.sha256)?;
            }
        }
        Ok(())
    }
}

impl Saved {
    pub fn check(&self) -> Result<(), String> {
        self.selection.check()?;
        digest(&self.archive.sha256)?;
        if self.archive.path != format!("packages/{}.tar.gz", self.archive.sha256) {
            return Err("saved fixture archive is not content addressed".into());
        }
        Ok(())
    }

    pub fn write(&self, store: &Store) -> Result<(), String> {
        self.check()?;
        crate::commit::durable(
            &store.root().join("fixtures").join(format!("{}.json", self.selection.package)),
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
        if entry.file_name().to_str() != Some(format!("{}.json", record.selection.package).as_str()) {
            return Err("saved fixture record belongs to another package".into());
        }
        saved.push(record);
    }
    saved.sort_by(|left, right| left.selection.package.cmp(&right.selection.package));
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
            inputs: vec![crate::engine::InputRecord {
                kind: crate::engine::InputKind::Snapshot,
                name: "fixture-osm".into(),
                digest: crate::engine::digest([("crop.osm.pbf", files[0].sha256.as_str())]),
                files: vec!["crop.osm.pbf".into()],
            }],
            options: serde_json::json!({}),
            code: "2".repeat(64),
            command: None,
            outputs: vec!["crop.osm.pbf".into()],
            digest: crate::engine::layer_digest(&files[..1], &[]),
            files: files[..1].to_vec(),
            snapshots: [("fixture-osm".into(), SnapshotRead { version: "1".into(), params: Vec::new() })].into(),
            client: Client::None,
        };
        let inputs = Inputs {
            terrain: Some(CapturedInput {
                source: "fixture-unused-terrain".into(),
                version: "1".into(),
                params: Vec::new(),
                files: vec!["unused.tif".into()],
            }),
            historical: BTreeMap::new(),
            osm: CapturedInput {
                source: "fixture-osm".into(),
                version: "1".into(),
                params: Vec::new(),
                files: vec!["crop.osm.pbf".into()],
            },
            osm_sha256: files[0].sha256.clone(),
            content: BTreeMap::new(),
            empty: BTreeMap::new(),
        };
        let release = Release::compose("maps", "ride", &[], None, vec![layer], &Default::default());
        let env = Env { moves: [("copernicus-glo-30".into(), Some("2022-05-09".into()))].into(), ..Default::default() };
        let selection =
            Selection::new(&store, "ride".into(), [0., 0., 1., 1.], inputs, release, &env, BTreeMap::new()).unwrap();
        assert!(selection.inputs.terrain.is_none(), "unused terrain is absent, never an empty all-files selector");
        let selected = Saved {
            selection,
            archive: LayerFile { path: format!("packages/{}.tar.gz", files[0].sha256), ..files[0].clone() },
        };
        selected.write(&store).unwrap();
        let recovered = Selection::local(&store, "ride").unwrap().unwrap();
        assert_eq!(recovered.selection.identity().unwrap(), selected.selection.identity().unwrap());
        assert_eq!(recovered.selection.environment("ride").version("fixture-osm", &[]).unwrap(), Some("1"));
        std::fs::create_dir_all(scratch.0.join("fixtures")).unwrap();
        std::fs::write(scratch.0.join("fixtures/catalog.toml"), format!("schema = 1\nbase_url = \"https://fixtures.invalid/v1/\"\n[packages.ride]\nselection = true\narchive = \"{}\"\nsha256 = \"{}\"\nbytes = {}\n", selected.archive.path, selected.archive.sha256, selected.archive.size)).unwrap();
        let catalog = Catalog::read(&scratch.0).unwrap();
        assert_eq!(
            Selection::basis(&store, &catalog, "ride").unwrap().unwrap().selection.identity().unwrap(),
            selected.selection.identity().unwrap()
        );
        assert!(Selection::basis(&Store::at(scratch.0.join("unprepared")), &catalog, "ride")
            .unwrap_err()
            .contains("explicit preparation"));
        let remote = scratch.0.join("bucket");
        let copy = remote.join(format!("inputs/objects/{}", files[0].sha256));
        std::fs::create_dir_all(copy.parent().unwrap()).unwrap();
        std::fs::copy(store.object(&files[0].sha256), &copy).unwrap();
        let cold = Store::at(scratch.0.join("cold"));
        let remote = crate::live::Remote::Bucket(crate::r2::Bucket::local(&remote));
        recovered.selection.restore(&cold, &crate::fetch::http::Http::new(), &remote, &BTreeMap::new()).unwrap();
        assert_eq!(std::fs::read(cold.object(&files[0].sha256)).unwrap(), b"old");
        assert!(cold.snapshot("fixture-osm", "2").unwrap().is_none());
        let missing = Store::at(scratch.0.join("missing"));
        std::fs::remove_file(copy).unwrap();
        assert!(recovered
            .selection
            .restore(&missing, &crate::fetch::http::Http::new(), &remote, &BTreeMap::new())
            .is_err());
        assert!(missing.snapshot("fixture-osm", "1").unwrap().is_none());
        recovered
            .selection
            .restore(
                &missing,
                &crate::fetch::http::Http::new(),
                &remote,
                &[("fixture-osm".into(), Some("2".into()))].into(),
            )
            .unwrap();
        assert!(
            missing.snapshot("fixture-osm", "1").unwrap().is_none(),
            "an explicit refresh does not require the lost old copy"
        );
        let plan = gc::plan(&store, &gc::Roots::default()).unwrap();
        assert!(!plan.snapshots.contains(&"fixture-osm@1".into()));
        assert!(plan.kept.iter().any(|kept| kept.entry == "fixture-osm@1" && kept.because == ["fixture ride"]));
        assert!(!plan.objects.iter().any(|(hash, _)| hash == &files[0].sha256));
        std::fs::write(store.root().join("fixtures/ride.json"), b"invalid record").unwrap();
        assert!(gc::plan(&store, &gc::Roots::default()).is_err());
    }
}
