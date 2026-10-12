//! A release: the layers of one product for a region in a small manifest. Its id is the SHA-256
//! of the manifest, and the manifest holds no cost of a build, so two machines that build the same
//! layers give the same id.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::plan::walk;
use super::{sorted, Client, Input, InputRecord, LayerFile, Receipt, Step};
use crate::store::{self, sha256_hex, write_atomic, Store};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Release {
    pub product: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<crate::settings::Settings>,
    /// The region of the environment that the release was built for.
    pub region: String,
    /// The optional layers of the product that the environment switched on, sorted.
    pub optional: Vec<String>,
    /// Sorted by `step`.
    pub layers: Vec<Layer>,
    /// Exact files under `releases/<id>/`, sorted by path. Their identity is part of the release id.
    pub named: Vec<LayerFile>,
}

/// The role of a published file. A named file retains its product-declared alias.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Published<'a> {
    Manifest,
    Named(&'a str),
    Object,
}

/// Immutable publication paths, relative to the product prefix.
pub struct Publication<'a> {
    release: &'a Release,
    manifest: LayerFile,
}

impl Publication<'_> {
    fn named_path(&self, name: &str) -> String {
        format!("releases/{}/{name}", self.manifest.sha256)
    }

    fn object_path(sha256: &str) -> String {
        format!("objects/{sha256}")
    }

    pub fn files(&self) -> impl Iterator<Item = (Published<'_>, LayerFile)> {
        let manifest = std::iter::once((Published::Manifest, self.manifest.clone()));
        let named = self.release.named.iter().map(|file| {
            (Published::Named(file.path.as_str()), LayerFile { path: self.named_path(&file.path), ..file.clone() })
        });
        let objects = self.release.objects().into_iter().map(|(sha256, size)| {
            (Published::Object, LayerFile { path: Self::object_path(sha256), sha256: sha256.into(), size })
        });
        manifest.chain(named).chain(objects)
    }

    /// The published address of a selected layer file, or none for an unpublished intermediate.
    pub fn path(&self, layer: &Layer, file: &LayerFile) -> Option<String> {
        if layer.client.includes(&file.path) {
            return Some(Self::object_path(&file.sha256));
        }
        self.release
            .named
            .iter()
            .find(|named| (named.size, &named.sha256) == (file.size, &file.sha256))
            .map(|named| self.named_path(&named.path))
    }
}

/// The files of a step in a release: its receipt without the cost fields, and what it read of
/// each source.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Layer {
    pub layer: String,
    pub step: String,
    pub format: u32,
    pub revision: u32,
    pub key: String,
    pub inputs: Vec<InputRecord>,
    pub options: Value,
    pub digest: String,
    pub files: Vec<LayerFile>,
    /// Source id to the fetch that the layer read: a receipt holds digests, not versions.
    pub snapshots: BTreeMap<String, SnapshotRead>,
    /// The published outputs. The release records every file for provenance.
    pub client: Client,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotRead {
    pub version: String,
    /// The `NAME=VALUE` of the fetch, sorted.
    pub params: Vec<(String, String)>,
}

impl Layer {
    /// The layer that `receipt` records, built by `step`.
    pub fn new(receipt: &Receipt, step: &Step) -> Self {
        let snapshots = step.inputs.iter().filter_map(|input| match input {
            Input::Snapshot { source, version, params, .. } => {
                Some((source.clone(), SnapshotRead { version: version.clone(), params: store::sorted(params) }))
            }
            Input::Layer { .. } => None,
        });
        Layer {
            layer: receipt.layer.clone(),
            step: receipt.step.clone(),
            format: receipt.format,
            revision: receipt.revision,
            key: receipt.key.clone(),
            inputs: receipt.inputs.clone(),
            options: receipt.options.clone(),
            digest: receipt.digest.clone(),
            files: receipt.files.clone(),
            snapshots: snapshots.collect(),
            client: step.client.sorted(),
        }
    }

    /// Only these files reach R2. All publication and reachability checks use this selection.
    pub fn client_files(&self) -> impl Iterator<Item = &LayerFile> {
        self.files.iter().filter(|file| self.client.includes(&file.path))
    }
}

impl Release {
    /// The release that a plan of `live` gives: the layers of `live` that `new` does not replace
    /// and that `dropped` does not name, and `new`.
    pub fn compose(
        product: &str,
        region: &str,
        optional: &[String],
        live: Option<&Release>,
        new: Vec<Layer>,
        dropped: &BTreeSet<String>,
    ) -> Release {
        let replaced = |layer: &&Layer| new.iter().any(|n| n.step == layer.step) || dropped.contains(&layer.step);
        let kept: Vec<Layer> =
            live.into_iter().flat_map(|live| &live.layers).filter(|layer| !replaced(layer)).cloned().collect();
        let mut layers: Vec<Layer> = kept.into_iter().chain(new).collect();
        layers.sort_by(|a, b| a.step.cmp(&b.step));
        let mut optional = optional.to_vec();
        optional.sort();
        Release { settings: None, product: product.into(), region: region.into(), optional, layers, named: Vec::new() }
    }

    /// Finalize the named publication files from receipt metadata before calculating the id.
    pub fn name_files(&mut self, mut files: Vec<LayerFile>) -> Result<(), String> {
        files.sort_by(|a, b| a.path.cmp(&b.path));
        self.named = files;
        self.check_named()
    }

    pub fn check_named(&self) -> Result<(), String> {
        for (at, file) in self.named.iter().enumerate() {
            if file.path.contains('\\') || file.path.split('/').any(|part| matches!(part, "" | "." | "..")) {
                return Err(format!("named file `{}` is not a normalized relative path", file.path));
            }
            if file.sha256.len() != 64 || !file.sha256.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(format!("named file `{}` has no SHA-256", file.path));
            }
            if at > 0 && self.named[at - 1].path >= file.path {
                return Err("named files have duplicate or unsorted paths".into());
            }
            if !self
                .layers
                .iter()
                .flat_map(|layer| &layer.files)
                .any(|stored| (stored.size, &stored.sha256) == (file.size, &file.sha256))
            {
                return Err(format!("named file `{}` has no receipt", file.path));
            }
        }
        Ok(())
    }

    /// The manifest as compact JSON with the keys of each object in byte order.
    pub(crate) fn canonical(&self) -> Vec<u8> {
        let value = serde_json::to_value(self).expect("a release serializes");
        serde_json::to_vec(&sorted(value)).expect("JSON values serialize")
    }

    pub fn id(&self) -> String {
        sha256_hex(&self.canonical())
    }

    /// Use the written or verified manifest id; decoding JSON numbers can change their representation.
    pub fn publication(&self, id: &str) -> Publication<'_> {
        let bytes = self.canonical();
        let manifest = LayerFile { path: format!("releases/{id}.json"), sha256: id.into(), size: bytes.len() as u64 };
        Publication { release: self, manifest }
    }

    /// The objects of its client layers, which R2 holds: SHA-256 to size.
    pub fn objects(&self) -> BTreeMap<&str, u64> {
        let files = self.layers.iter().flat_map(Layer::client_files);
        files.map(|file| (file.sha256.as_str(), file.size)).collect()
    }

    /// Write the manifest to `releases/<product>/<id>.json` in its canonical form, so the SHA-256
    /// of the file is the id. Returns the id.
    pub fn write(&self, store: &Store) -> Result<String, String> {
        self.check_named()?;
        let id = self.id();
        write_atomic(&store.release(&self.product, &id), &self.canonical())?;
        Ok(id)
    }

    pub fn read(store: &Store, product: &str, id: &str) -> Result<Release, String> {
        let path = store.release(product, id);
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let release: Release = serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        release.check_named()?;
        Ok(release)
    }
}

/// The layer that the store has for each step that `names` names, by step. `steps` are the steps of
/// every product: a layer can read the layer of another product.
pub fn stored(store: &Store, steps: &[Step], names: &BTreeSet<&str>) -> Result<BTreeMap<String, Layer>, String> {
    let walked = walk(store, steps)?;
    let named = walked.iter().filter(|walked| names.contains(walked.step.name.as_str()));
    let layers = named.filter_map(|walked| walked.stored.clone());
    Ok(layers.map(|layer| (layer.step.clone(), layer)).collect())
}

/// The release of `product` for `region` when the store has the layer of each of its steps, the
/// steps whose name starts with `<product>/`.
pub fn release(
    store: &Store,
    product: &str,
    region: &str,
    optional: &[String],
    steps: &[Step],
) -> Result<Option<Release>, String> {
    let prefix = format!("{product}/");
    let names: BTreeSet<&str> =
        steps.iter().map(|step| step.name.as_str()).filter(|n| n.starts_with(&prefix)).collect();
    let layers = stored(store, steps, &names)?;
    if layers.len() < names.len() {
        return Ok(None);
    }
    Ok(Some(Release::compose(product, region, optional, None, layers.into_values().collect(), &BTreeSet::new())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::tests::{fixture, pipeline};
    use crate::store::hash_file;

    #[test]
    fn named_files_bind_identity_and_require_normalized_unique_receipt_paths() {
        let fixture = fixture("release-named");
        fixture.build(&pipeline()).unwrap();
        let mut release = release(&fixture.store, "test", "monaco", &[], &pipeline()).unwrap().unwrap();
        let original = release.id();
        let mut file = release.layers[0].files[0].clone();
        file.path = "regions/monaco.json".into();
        release.name_files(vec![file.clone()]).unwrap();
        let named = release.id();
        assert_ne!(named, original, "the named identity is in the manifest");
        file.path = "regions/andorra.json".into();
        release.name_files(vec![file.clone()]).unwrap();
        assert_ne!(release.id(), named, "the published path is immutable too");
        assert!(release.name_files(vec![file.clone(), file.clone()]).unwrap_err().contains("duplicate"));
        for path in ["", "/schema.json", "../schema.json", "regions/./x", "regions//x", "regions\\x"] {
            file.path = path.into();
            assert!(release.name_files(vec![file.clone()]).unwrap_err().contains("normalized"), "{path}");
        }
        file.path = "schema.json".into();
        file.sha256 = "a".repeat(64);
        assert!(release.name_files(vec![file]).unwrap_err().contains("no receipt"));
    }

    #[test]
    fn selected_client_outputs_reuse_bytes_keep_provenance_and_deduplicate_objects() {
        let fixture = fixture("release-selection");
        let mut steps = vec![crate::engine::tests::packaged(Client::All)];
        fixture.build(&steps).unwrap();
        let all = release(&fixture.store, "test", "monaco", &[], &steps).unwrap().unwrap();
        let before = super::super::recipe(&steps[0]);
        steps[0].client = Client::Paths(vec!["published".into()]);
        assert_ne!(super::super::recipe(&steps[0]), before);
        assert!(fixture.plan(&steps).unwrap().groups.is_empty(), "visibility changes reuse byte artifacts");
        let mut selected = release(&fixture.store, "test", "monaco", &[], &steps).unwrap().unwrap();
        assert_eq!(selected.layers[0].key, all.layers[0].key);
        assert_eq!(selected.layers[0].files.len(), 4, "private files stay in provenance");
        assert_eq!(
            selected.layers[0].client_files().map(|file| file.path.as_str()).collect::<Vec<_>>(),
            ["published/a", "published/b"]
        );
        assert_eq!(selected.objects().len(), 1, "equal selected payloads share one object");
        assert_ne!(selected.id(), all.id());

        let metadata = selected.layers[0].files.iter().find(|file| file.path == "index.json").unwrap().clone();
        selected
            .name_files(
                ["indexes/a.json", "indexes/b.json"]
                    .map(|path| LayerFile { path: path.into(), ..metadata.clone() })
                    .into(),
            )
            .unwrap();
        let before = selected.canonical();
        let id = selected.id();
        let publication = selected.publication(&id);
        let published: Vec<_> = publication.files().collect();
        let payload = selected.layers[0].files.iter().find(|file| file.path == "published/a").unwrap();
        assert_eq!(
            published,
            vec![
                (
                    Published::Manifest,
                    LayerFile { path: format!("releases/{id}.json"), sha256: id.clone(), size: before.len() as u64 }
                ),
                (
                    Published::Named("indexes/a.json"),
                    LayerFile { path: format!("releases/{id}/indexes/a.json"), ..metadata.clone() }
                ),
                (
                    Published::Named("indexes/b.json"),
                    LayerFile { path: format!("releases/{id}/indexes/b.json"), ..metadata.clone() }
                ),
                (Published::Object, LayerFile { path: format!("objects/{}", payload.sha256), ..payload.clone() }),
            ]
        );
        let layer = &selected.layers[0];
        assert_eq!(publication.path(layer, payload), Some(format!("objects/{}", payload.sha256)));
        assert_eq!(publication.path(layer, &metadata), Some(format!("releases/{id}/indexes/a.json")));
        let private = layer.files.iter().find(|file| file.path == "published-extra/a").unwrap();
        assert_eq!(publication.path(layer, private), None);
        assert_eq!(publication.path(layer, &LayerFile { size: metadata.size + 1, ..metadata.clone() }), None);
        assert_eq!(selected.canonical(), before, "publication does not change the manifest or its identity");

        steps[0].client = Client::Paths(vec!["published/a".into()]);
        assert!(fixture.plan(&steps).unwrap_err().contains("not a declared output"));
        steps[0].client = Client::Paths(Vec::new());
        assert!(fixture.plan(&steps).unwrap_err().contains("client paths is empty"));
    }

    #[test]
    fn two_stores_that_build_the_same_layers_give_the_same_release() {
        let (one, two) = (fixture("release-one"), fixture("release-two"));
        assert_eq!(release(&one.store, "test", "monaco", &[], &pipeline()).unwrap(), None, "nothing is built");

        let built = one.build(&pipeline()).unwrap();
        two.build(&pipeline()).unwrap();
        let first = release(&one.store, "test", "monaco", &[], &pipeline()).unwrap().unwrap();
        let second = release(&two.store, "test", "monaco", &[], &pipeline()).unwrap().unwrap();
        assert_eq!(first.id(), second.id(), "the cost of a build is not in a release");

        let steps: Vec<&str> = first.layers.iter().map(|layer| layer.step.as_str()).collect();
        assert_eq!(steps, ["test/count", "test/join", "test/upper"]);
        assert_eq!(first.layers[1].snapshots["tail"], SnapshotRead { version: "1".into(), params: Vec::new() });
        let objects: Vec<(&str, u64)> = first.objects().into_iter().collect();
        let mut expected: Vec<(&str, u64)> =
            built.iter().flat_map(|b| &b.receipt.files).map(|file| (file.sha256.as_str(), file.size)).collect();
        expected.sort();
        assert_eq!(objects, expected);

        let mut steps = pipeline();
        steps.iter_mut().filter(|step| step.name == "test/join").for_each(|step| step.client = Client::None);
        let lean = release(&one.store, "test", "monaco", &[], &steps).unwrap().unwrap();
        assert_eq!(lean.layers.len(), 3, "the release records an intermediate layer");
        let mut client = first.objects();
        client.retain(|sha256, _| first.layers[1].files.iter().all(|file| file.sha256 != *sha256));
        assert_eq!(lean.objects(), client, "R2 lacks the files of an intermediate layer");

        let id = first.write(&one.store).unwrap();
        assert_eq!(hash_file(&one.store.release("test", &id)).unwrap().0, id, "the file is canonical");
        assert_eq!(Release::read(&one.store, "test", &id).unwrap(), first);

        let join = Layer { key: "another".into(), ..first.layers[1].clone() };
        let dropped = BTreeSet::from(["test/count".to_string()]);
        let next = Release::compose("test", "andorra", &[], Some(&first), vec![join.clone()], &dropped);
        assert_eq!(next.layers, [join, first.layers[2].clone()], "the new layer, and the live layer that stays");
        assert_eq!(next.region, "andorra");

        let mut steps = pipeline();
        steps[2].options = serde_json::json!({"case": "upper"});
        assert_eq!(release(&one.store, "test", "monaco", &[], &steps).unwrap(), None, "a key that the store lacks");
    }
}
