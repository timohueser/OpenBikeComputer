//! Verified portable data keeps its original release and producer provenance. It is not a
//! receipt of a build on this host. Saved adoptions remain store roots when no app runs.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::engine::release::{Layer, Release};
use crate::engine::{self, Code, Input, InputKind, LayerFile, ResolvedRust, Run, SourceIdentity, Step};
use crate::fetch::http::{Expect, Http};
use crate::live::Remote;
use crate::product::{BlockedLayer, Product};
use crate::store::{hash_file, sorted, Store};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub step: String,
    pub files: Vec<LayerFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub product: String,
    pub release: String,
    pub layers: Vec<Selection>,
    pub blocked: Vec<BlockedLayer>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Adoption {
    pub original: Release,
    pub plan: Plan,
}

/// Compare declared producers at the original target/profile, without selecting execution tools.
/// Each requested layer includes its original published client files and the exact extra paths in `required`.
/// A private intermediate is eligible only when its verified bytes already exist locally.
pub fn plan(
    root: &Path,
    store: &Store,
    product: &dyn Product,
    release: &Release,
    steps: &[Step],
    required: &BTreeMap<String, Vec<String>>,
) -> Result<Plan, String> {
    release.check_named()?;
    if release.product != product.name() || !crate::is_kebab(product.name()) {
        return Err("the portable release belongs to another product".into());
    }
    let mut check = Compatibility { root, store, release, steps, checked: BTreeMap::new(), codes: Vec::new() };
    let mut plan =
        Plan { product: release.product.clone(), release: release.id(), layers: Vec::new(), blocked: Vec::new() };
    for (name, extra) in required {
        let mut select = || -> Result<Selection, String> {
            let step = steps.iter().find(|step| &step.name == name).ok_or("no current producer declaration")?;
            if !product.portable(step) {
                return Err("this owner does not declare portable data for the layer".into());
            }
            let layer = release.layers.iter().find(|layer| &layer.step == name).ok_or("the release lacks the layer")?;
            check.layer(name, &mut BTreeSet::new())?;
            if extra.iter().any(|path| !layer.files.iter().any(|file| &file.path == path)) {
                return Err("the release lacks a required file".into());
            }
            let files: Vec<_> = layer
                .files
                .iter()
                .filter(|file| layer.client.includes(&file.path) || extra.contains(&file.path))
                .cloned()
                .collect();
            for file in &files {
                if store.object(&file.sha256).is_file() {
                    verify(store, file)?;
                } else if remote_key(product.prefix(), release, layer, file).is_none() {
                    return Err(format!("{} is an unpublished input; rebuild or materialize it locally", file.path));
                }
            }
            Ok(Selection { step: name.clone(), files })
        };
        match select() {
            Ok(selection) => plan.layers.push(selection),
            Err(reason) => plan.blocked.push(BlockedLayer { layer: name.clone(), reason }),
        }
    }
    Ok(plan)
}

struct Compatibility<'a> {
    root: &'a Path,
    store: &'a Store,
    release: &'a Release,
    steps: &'a [Step],
    checked: BTreeMap<String, Result<(), String>>,
    codes: Vec<(Code, Option<ResolvedRust>, SourceIdentity)>,
}

impl Compatibility<'_> {
    fn layer(&mut self, name: &str, visiting: &mut BTreeSet<String>) -> Result<(), String> {
        if let Some(result) = self.checked.get(name) {
            return result.clone();
        }
        if !visiting.insert(name.into()) {
            return Err(format!("portable inputs form a cycle at {name}"));
        }
        let result = self.compare(name, visiting);
        visiting.remove(name);
        self.checked.insert(name.into(), result.clone());
        result.map_err(|error| format!("{name}: {error}"))
    }

    fn compare(&mut self, name: &str, visiting: &mut BTreeSet<String>) -> Result<(), String> {
        let (steps, release) = (self.steps, self.release);
        let step = steps.iter().find(|step| step.name == name).ok_or("no current input producer")?;
        let layer = release.layers.iter().find(|layer| layer.step == name).ok_or("no original input layer")?;
        check_files(&layer.files)?;
        if engine::layer_digest(&layer.files, &[]) != layer.digest {
            return Err("the original output digest differs from its files".into());
        }
        let producer = self.release.producers.get(&layer.code).ok_or("the original producer witness is unavailable")?;
        producer.check(&layer.code)?;
        let at = match self.codes.iter().position(|(code, rust, _)| code == &step.code && rust == &producer.rust) {
            Some(at) => at,
            None => {
                let identity = step.code.source_config(self.root, producer.rust.as_ref())?;
                self.codes.push((step.code.clone(), producer.rust.clone(), identity));
                self.codes.len() - 1
            }
        };
        let identity = &self.codes[at].2;
        if identity.files != producer.source_config || identity.rust != producer.rust {
            return Err("the source/config differs at the original target and profile".into());
        }
        let command = match &step.run {
            Run::Rust(_) => None,
            Run::Command(argv) => Some(argv.clone()),
        };
        let mut outputs = step.outputs.clone();
        outputs.sort();
        if (step.options.clone(), command, outputs)
            != (layer.options.clone(), layer.command.clone(), layer.outputs.clone())
        {
            return Err("the producer options, command or outputs differ".into());
        }
        if step.inputs.len() != layer.inputs.len() {
            return Err("the input selection differs".into());
        }
        let mut selected = BTreeSet::new();
        for input in &step.inputs {
            let (kind, name) = match input {
                Input::Snapshot { source, .. } => (InputKind::Snapshot, source),
                Input::Layer { name, .. } => (InputKind::Layer, name),
            };
            if !selected.insert((kind, name)) {
                return Err("the producer repeats an input".into());
            }
            let record = layer
                .inputs
                .iter()
                .find(|record| (record.kind, &record.name) == (kind, name))
                .ok_or("the input selection differs")?;
            match input {
                Input::Layer { name, files } => {
                    let mut files = files.clone();
                    files.sort();
                    if files != record.files {
                        return Err("the selected layer paths differ".into());
                    }
                    self.layer(name, visiting)?;
                    let original = self.release.layers.iter().find(|layer| &layer.step == name).expect("checked input");
                    if files.iter().any(|path| !original.files.iter().any(|file| &file.path == path))
                        || engine::layer_digest(&original.files, &files) != record.digest
                    {
                        return Err("the original selected layer bytes differ".into());
                    }
                }
                Input::Snapshot { source, version, params, files } => {
                    let read = layer.snapshots.get(source).ok_or("the original snapshot request is unavailable")?;
                    if read.version != *version || read.params != sorted(params) {
                        return Err("the exact snapshot version or request differs".into());
                    }
                    let mut files = files.clone();
                    files.sort();
                    if !files.is_empty() && files != record.files {
                        return Err("the selected snapshot paths differ".into());
                    }
                    if !params.is_empty()
                        && self.store.requested(source, version, params)?.is_some_and(|mut names| {
                            names.sort();
                            names != record.files
                        })
                    {
                        return Err("the retained request selects other snapshot files".into());
                    }
                    if let Some(snapshot) = self.store.snapshot(source, version)? {
                        if params.is_empty() && files.is_empty() && snapshot.files.len() != record.files.len() {
                            return Err("the complete snapshot now selects other files".into());
                        }
                        let files: Vec<_> =
                            snapshot.files.iter().filter(|file| record.files.contains(&file.name)).collect();
                        if files.len() != record.files.len()
                            || engine::digest(files.iter().map(|file| (file.name.as_str(), file.sha256.as_str())))
                                != record.digest
                        {
                            return Err("the selected snapshot bytes differ".into());
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

fn remote_key(prefix: &str, release: &Release, layer: &Layer, file: &LayerFile) -> Option<String> {
    if layer.client.includes(&file.path) {
        return Some(format!("{prefix}/objects/{}", file.sha256));
    }
    release
        .named
        .iter()
        .find(|named| (named.size, &named.sha256) == (file.size, &file.sha256))
        .map(|named| format!("{prefix}/releases/{}/{}", release.id(), named.path))
}

fn verify(store: &Store, file: &LayerFile) -> Result<(), String> {
    if hash_file(&store.object(&file.sha256))? != (file.sha256.clone(), file.size) {
        return Err(format!("{}: portable bytes have another SHA-256 or size", file.path));
    }
    Ok(())
}

/// Accept only the reviewed selection. Verify every file and recheck source/config before
/// replacing the saved anchor. A failed transfer leaves verified cache bytes, not a new anchor.
#[allow(clippy::too_many_arguments)]
pub fn adopt(
    root: &Path,
    store: &Store,
    remote: &Remote,
    product: &dyn Product,
    release: &Release,
    steps: &[Step],
    required: &BTreeMap<String, Vec<String>>,
    confirmed: &Plan,
) -> Result<Adoption, String> {
    let _using = store.using()?;
    let _anchor = store.lock(&format!("local-{}", product.name()))?;
    let current = plan(root, store, product, release, steps, required)?;
    if &current != confirmed || !current.blocked.is_empty() || current.layers.is_empty() {
        return Err("portable adoption changed or is blocked; review it again".into());
    }
    for selection in &current.layers {
        let layer = release.layers.iter().find(|layer| layer.step == selection.step).expect("planned layer");
        for file in &selection.files {
            let _object = store.lock(&format!("local-object-{}", file.sha256))?;
            if !store.object(&file.sha256).is_file() {
                let key =
                    remote_key(product.prefix(), release, layer, file).ok_or("the required input is unpublished")?;
                match remote {
                    Remote::Bucket(bucket) => {
                        let part = store.partial(&format!("local-{}.part", file.sha256));
                        std::fs::create_dir_all(part.parent().expect("partial directory"))
                            .map_err(|e| e.to_string())?;
                        bucket.get(&key, &part)?;
                        if hash_file(&part)? != (file.sha256.clone(), file.size) {
                            let _ = std::fs::remove_file(&part);
                            return Err(format!("{key}: portable bytes have another SHA-256 or size"));
                        }
                        store.insert(&part, &file.sha256)?;
                    }
                    Remote::Public(url) => {
                        let url = format!("{url}/{key}");
                        let _lock = Http::lock(store, &url)?;
                        Http::new().download(
                            store,
                            &url,
                            &Expect { sha256: Some(&file.sha256), ..Expect::default() },
                        )?;
                    }
                }
            }
            verify(store, file)?;
        }
    }
    if plan(root, store, product, release, steps, required)? != current {
        return Err("portable provenance changed during adoption; review it again".into());
    }
    let adoption = Adoption { original: release.clone(), plan: current };
    store.put_adoption(&adoption)?;
    Ok(adoption)
}

pub fn saved(store: &Store) -> Result<Vec<Adoption>, String> {
    let adoptions = store.adoptions()?;
    for adoption in &adoptions {
        adoption.check()?;
    }
    Ok(adoptions)
}

fn check_files(files: &[LayerFile]) -> Result<(), String> {
    let mut paths = BTreeSet::new();
    for file in files {
        if file.path.contains(['\\', '\r', '\n'])
            || file.path.split('/').any(|part| matches!(part, "" | "." | ".."))
            || !paths.insert(&file.path)
            || file.sha256.len() != 64
            || !file.sha256.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("portable files need unique relative paths and SHA-256 digests".into());
        }
    }
    Ok(())
}

impl Adoption {
    fn check(&self) -> Result<(), String> {
        self.original.check_named()?;
        if !crate::is_kebab(&self.plan.product)
            || self.plan.product != self.original.product
            || self.plan.release != self.original.id()
            || !self.plan.blocked.is_empty()
            || self.plan.layers.is_empty()
        {
            return Err("saved Local adoption has another original release or incomplete selection".into());
        }
        let mut names = BTreeSet::new();
        for selection in &self.plan.layers {
            check_files(&selection.files)?;
            let layer = self
                .original
                .layers
                .iter()
                .find(|layer| layer.step == selection.step)
                .ok_or("saved Local selection has no original layer")?;
            if !names.insert(&selection.step) || selection.files.iter().any(|file| !layer.files.contains(file)) {
                return Err("saved Local selection differs from its original layer".into());
            }
        }
        Ok(())
    }

    pub(crate) fn inputs(&self) -> Result<BTreeSet<&str>, String> {
        let mut reached = BTreeSet::new();
        let mut pending: Vec<_> = self.plan.layers.iter().map(|layer| layer.step.as_str()).collect();
        while let Some(name) = pending.pop() {
            if !reached.insert(name) {
                continue;
            }
            let layer = self
                .original
                .layers
                .iter()
                .find(|layer| layer.step == name)
                .ok_or("saved Local provenance lacks an input layer")?;
            pending.extend(
                layer.inputs.iter().filter(|input| input.kind == InputKind::Layer).map(|input| input.name.as_str()),
            );
        }
        Ok(reached)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::release::{Producer, SnapshotRead};
    use crate::engine::tests::{fixture, repository, write};
    use crate::engine::{Client, InputRecord, Library, Profile, Rust};
    use crate::product::{Steps, Unplanned};
    use crate::store::{sha256_hex, FileRecord, Snapshot};
    use serde_json::json;

    struct Portable;
    impl Product for Portable {
        fn name(&self) -> &'static str {
            "test"
        }
        fn portable(&self, step: &Step) -> bool {
            step.name == "test/client"
        }
        fn steps(
            &self,
            _: &Path,
            _: &crate::env::Env,
            _: &crate::regions::Regions,
            _: &Store,
        ) -> Result<Steps, Unplanned> {
            unreachable!("adoption compares the supplied declarations")
        }
    }

    fn file(path: &str, bytes: &[u8]) -> LayerFile {
        LayerFile { path: path.into(), size: bytes.len() as u64, sha256: sha256_hex(bytes) }
    }

    fn object(store: &Store, file: &LayerFile, bytes: &[u8]) {
        let part = store.partial(&file.sha256);
        std::fs::create_dir_all(part.parent().unwrap()).unwrap();
        std::fs::write(&part, bytes).unwrap();
        store.insert(&part, &file.sha256).unwrap();
    }

    fn authored(root: &Path) -> (Vec<Step>, Release) {
        repository(
            root,
            &[
                ("steps", "[target.'cfg(target_os = \"linux\")'.dependencies]\nlinux = { path = \"../linux\" }\n"),
                ("linux", ""),
            ],
        );
        let code = Code {
            crates: vec!["steps".into()],
            libraries: vec![Library {
                name: "original-provider".into(),
                path: root.join("absent-original-provider"),
                sha256: "a".repeat(64),
            }],
            ..Default::default()
        };
        let recorded =
            ResolvedRust { target: "x86_64-unknown-linux-gnu".into(), build: Rust::Native { profile: Profile::Dev } };
        let witness = code.source_config(root, Some(&recorded)).unwrap();
        assert!(witness.files.contains_key("linux/src/lib.rs"), "the original Linux dependency is selected");
        let mut full = witness.files.clone();
        full.insert("native/library/original-provider".into(), "a".repeat(64));
        full.insert(engine::code::SOURCE_BINDING.into(), engine::code::source_binding(&witness.files, Some(&recorded)));
        let producer = Producer { files: full, source_config: witness.files, rust: Some(recorded) };
        let code_digest = engine::code::hash(&producer.files);
        let steps = vec![
            crate::engine::tests::step(
                "test/input",
                vec![Input::Snapshot {
                    source: "land".into(),
                    version: "1".into(),
                    params: vec![],
                    files: vec!["land.bin".into()],
                }],
                code.clone(),
                "input.dat",
                Run::Rust(engine::pass),
            ),
            crate::engine::tests::step(
                "test/client",
                vec![Input::layer("test/input")],
                code,
                "client.bin",
                Run::Rust(engine::pass),
            ),
        ];
        let layers = [
            (
                "test/input",
                file("input.dat", b"private"),
                InputRecord {
                    kind: InputKind::Snapshot,
                    name: "land".into(),
                    digest: engine::digest([("land.bin", sha256_hex(b"land old").as_str())]),
                    files: vec!["land.bin".into()],
                },
            ),
            (
                "test/client",
                file("client.bin", b"client"),
                InputRecord {
                    kind: InputKind::Layer,
                    name: "test/input".into(),
                    digest: engine::digest([("input.dat", sha256_hex(b"private").as_str())]),
                    files: Vec::new(),
                },
            ),
        ]
        .into_iter()
        .map(|(name, file, input)| Layer {
            step: name.into(),
            key: sha256_hex(format!("original Linux {name}").as_bytes()),
            inputs: vec![input],
            options: json!({}),
            code: code_digest.clone(),
            command: None,
            outputs: vec![file.path.clone()],
            digest: engine::layer_digest(std::slice::from_ref(&file), &[]),
            files: vec![file],
            snapshots: if name == "test/input" {
                BTreeMap::from([("land".into(), SnapshotRead { version: "1".into(), params: vec![] })])
            } else {
                BTreeMap::new()
            },
            client: if name == "test/client" { Client::All } else { Client::None },
        })
        .collect();
        (
            steps,
            Release {
                product: "test".into(),
                region: "monaco".into(),
                optional: vec![],
                layers,
                named: vec![],
                producers: BTreeMap::from([(code_digest, producer)]),
            },
        )
    }

    #[test]
    fn portable_selection_keeps_foreign_provenance_without_selecting_its_execution_tools() {
        let fixture = fixture("local-portable");
        let root = fixture.root();
        let (mut steps, original) = authored(&root);
        let required = BTreeMap::from([("test/client".into(), Vec::new())]);
        let producer = &original.producers[&original.layers[0].code];
        for mutation in 0..4 {
            let mut forged = producer.clone();
            match mutation {
                0 => {
                    forged.source_config.insert("linux/src/lib.rs".into(), "f".repeat(64));
                }
                1 => {
                    forged.source_config.insert("new/source-key".into(), "f".repeat(64));
                }
                2 => {
                    forged.rust.as_mut().unwrap().target = "aarch64-apple-darwin".into();
                }
                _ => {
                    forged.rust.as_mut().unwrap().build = Rust::Prepared { profile: Profile::Release };
                }
            }
            assert!(forged.check(&original.layers[0].code).unwrap_err().contains("source/config binding"));
            let path = fixture.store.root().join("producers").join(format!("{}.json", original.layers[0].code));
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, serde_json::to_vec(&forged).unwrap()).unwrap();
            assert!(original.clone().bind_producers(&fixture.store).unwrap_err().contains("source/config binding"));
        }
        for step in &mut steps {
            step.code.libraries[0].path = root.join("absent-local-provider");
            step.code.libraries[0].sha256 = "b".repeat(64);
        }
        let selected = plan(&root, &fixture.store, &Portable, &original, &steps, &required).unwrap();
        assert!(selected.blocked.is_empty(), "{selected:?}");
        assert!(!root.join("absent-original-provider").exists());
        let mut local_full = original.producers[&original.layers[0].code].files.clone();
        local_full.insert("native/library/original-provider".into(), "b".repeat(64));
        assert_ne!(engine::code::hash(&local_full), original.layers[0].code, "a Local execution has its own full key");
        object(&fixture.store, &original.layers[1].files[0], b"client");
        let adopted = adopt(
            &root,
            &fixture.store,
            &Remote::Public("https://unused.invalid".into()),
            &Portable,
            &original,
            &steps,
            &required,
            &selected,
        )
        .unwrap();
        assert_eq!(adopted.original, original);
        assert_eq!(saved(&fixture.store).unwrap(), [adopted]);
        assert!(fixture.store.layers().unwrap().is_empty(), "adoption does not fabricate native receipts");
        steps[1].options = json!({"changed":true});
        assert!(plan(&root, &fixture.store, &Portable, &original, &steps, &required).unwrap().blocked[0]
            .reason
            .contains("options"));
        steps[1].options = json!({});
        let Input::Snapshot { version, .. } = &mut steps[0].inputs[0] else { unreachable!() };
        *version = "2".into();
        assert!(plan(&root, &fixture.store, &Portable, &original, &steps, &required).unwrap().blocked[0]
            .reason
            .contains("version"));
        *match &mut steps[0].inputs[0] {
            Input::Snapshot { version, .. } => version,
            _ => unreachable!(),
        } = "1".into();
        write(&root.join("linux/src/lib.rs"), "pub fn changed() {}\n");
        assert!(plan(&root, &fixture.store, &Portable, &original, &steps, &required).unwrap().blocked[0]
            .reason
            .contains("source/config"));
        assert_eq!(
            saved(&fixture.store).unwrap()[0].original,
            original,
            "saved Local data is not rewritten by checkout edits"
        );
    }

    #[test]
    fn transfer_verifies_files_and_stopped_local_data_remains_a_collection_root() {
        let fixture = fixture("local-roots");
        let root = fixture.root();
        let (mut steps, mut original) = authored(&root);
        let metadata = file("metadata.json", b"metadata");
        steps[1].outputs.push(metadata.path.clone());
        steps[1].client = Client::Paths(vec!["client.bin".into()]);
        original.layers[1].outputs.push(metadata.path.clone());
        original.layers[1].outputs.sort();
        original.layers[1].files.push(metadata.clone());
        original.layers[1].digest = engine::layer_digest(&original.layers[1].files, &[]);
        original.layers[1].client = steps[1].client.clone();
        let required = BTreeMap::from([("test/client".into(), vec![metadata.path.clone()])]);
        assert!(plan(&root, &fixture.store, &Portable, &original, &steps, &required).unwrap().blocked[0]
            .reason
            .contains("unpublished"));
        original.name_files(vec![LayerFile { path: "catalog.json".into(), ..metadata.clone() }]).unwrap();
        let selected = plan(&root, &fixture.store, &Portable, &original, &steps, &required).unwrap();
        let bucket = root.join("bucket");
        let key = format!("test/objects/{}", original.layers[1].files[0].sha256);
        write(&bucket.join(format!("test/releases/{}/catalog.json", original.id())), "metadata");
        write(&bucket.join(&key), "tamper");
        let remote = Remote::Bucket(crate::r2::Bucket::local(&bucket));
        assert!(adopt(&root, &fixture.store, &remote, &Portable, &original, &steps, &required, &selected)
            .unwrap_err()
            .contains("SHA-256"));
        assert!(saved(&fixture.store).unwrap().is_empty());
        write(&bucket.join(&key), "client");
        adopt(&root, &fixture.store, &remote, &Portable, &original, &steps, &required, &selected).unwrap();
        for (version, bytes) in [("1", &b"land old"[..]), ("2", &b"land new"[..])] {
            let file = file("land.bin", bytes);
            object(&fixture.store, &file, bytes);
            fixture
                .store
                .put_snapshot(&Snapshot {
                    source: "land".into(),
                    version: version.into(),
                    files: vec![FileRecord {
                        name: file.path,
                        url: "https://fixture.invalid/land".into(),
                        size: file.size,
                        sha256: file.sha256,
                        retrieved: format!("2026-10-0{version}T00:00:00Z"),
                    }],
                })
                .unwrap();
        }
        let garbage = file("unused", b"unused");
        object(&fixture.store, &garbage, b"unused");
        let roots = crate::store::gc::Roots::default();
        let cleanup = crate::store::gc::plan(&fixture.store, &roots).unwrap();
        assert!(cleanup.snapshots.is_empty(), "the stopped Local source version remains rooted");
        assert_eq!(cleanup.objects, [(garbage.sha256.clone(), garbage.size)]);
        assert!(cleanup.kept.iter().any(|kept| kept.entry == "land@1" && kept.because == ["local test"]));
        crate::store::gc::apply(&fixture.store, &roots, &cleanup).unwrap().unwrap();
        assert!(fixture.store.object(&original.layers[1].files[0].sha256).is_file());
        let mut tampered = saved(&fixture.store).unwrap().remove(0);
        tampered.plan.layers[0].files[0].sha256 = "c".repeat(64);
        fixture.store.put_adoption(&tampered).unwrap();
        assert!(
            crate::store::gc::plan(&fixture.store, &roots).unwrap_err().contains("selection differs"),
            "invalid roots fail closed"
        );
    }
}
