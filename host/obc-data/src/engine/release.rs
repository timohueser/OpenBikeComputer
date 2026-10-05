//! A release: the layers of one product in a small manifest. Its id is the SHA-256 of the
//! manifest, and the manifest holds no cost of a build, so two machines that build the same
//! layers give the same id.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::plan::walk;
use super::{sorted, Input, InputRecord, LayerFile, Receipt, Step};
use crate::store::{self, sha256_hex, write_atomic, Store};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Release {
    pub product: String,
    /// Sorted by `step`.
    pub layers: Vec<Layer>,
}

/// A layer of a release: its receipt without the cost fields, and what it read of each source.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Layer {
    pub step: String,
    pub key: String,
    pub inputs: Vec<InputRecord>,
    pub options: Value,
    pub code: String,
    pub command: Option<Vec<String>>,
    pub outputs: Vec<String>,
    pub digest: String,
    pub files: Vec<LayerFile>,
    /// Source id to the fetch that the layer read: a receipt holds digests, not versions.
    pub snapshots: BTreeMap<String, SnapshotRead>,
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
            step: receipt.step.clone(),
            key: receipt.key.clone(),
            inputs: receipt.inputs.clone(),
            options: receipt.options.clone(),
            code: receipt.code.clone(),
            command: receipt.command.clone(),
            outputs: receipt.outputs.clone(),
            digest: receipt.digest.clone(),
            files: receipt.files.clone(),
            snapshots: snapshots.collect(),
        }
    }
}

impl Release {
    /// The manifest as compact JSON with the keys of each object in byte order.
    fn canonical(&self) -> Vec<u8> {
        let value = serde_json::to_value(self).expect("a release serializes");
        serde_json::to_vec(&sorted(value)).expect("JSON values serialize")
    }

    pub fn id(&self) -> String {
        sha256_hex(&self.canonical())
    }

    /// The objects of its layers: SHA-256 to size.
    pub fn objects(&self) -> BTreeMap<&str, u64> {
        let files = self.layers.iter().flat_map(|layer| &layer.files);
        files.map(|file| (file.sha256.as_str(), file.size)).collect()
    }

    /// Write the manifest to `releases/<product>/<id>.json` in its canonical form, so the SHA-256
    /// of the file is the id. Returns the id.
    pub fn write(&self, store: &Store) -> Result<String, String> {
        let id = self.id();
        write_atomic(&store.release(&self.product, &id), &self.canonical())?;
        Ok(id)
    }

    pub fn read(store: &Store, product: &str, id: &str) -> Result<Release, String> {
        let path = store.release(product, id);
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// The release of `product` when the store has the layer of each of its steps, the steps whose
/// name starts with `<product>/`. `steps` are the steps of every product: a layer can read the
/// layer of another product.
pub fn release(store: &Store, root: &Path, product: &str, steps: &[Step]) -> Result<Option<Release>, String> {
    let prefix = format!("{product}/");
    let mut layers = Vec::new();
    for walked in walk(store, root, steps)?.iter().filter(|walked| walked.step.name.starts_with(&prefix)) {
        let Some(receipt) = &walked.stored else { return Ok(None) };
        layers.push(Layer::new(receipt, walked.step));
    }
    layers.sort_by(|a, b| a.step.cmp(&b.step));
    Ok(Some(Release { product: product.into(), layers }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::tests::{fixture, pipeline, write, JOIN};
    use crate::store::hash_file;

    #[test]
    fn two_stores_that_build_the_same_layers_give_the_same_release() {
        let (one, two) = (fixture("release-one"), fixture("release-two"));
        assert_eq!(release(&one.store, &one.root(), "test", &pipeline()).unwrap(), None, "nothing is built");

        let built = one.build(&pipeline()).unwrap();
        two.build(&pipeline()).unwrap();
        let first = release(&one.store, &one.root(), "test", &pipeline()).unwrap().unwrap();
        let second = release(&two.store, &two.root(), "test", &pipeline()).unwrap().unwrap();
        assert_eq!(first.id(), second.id(), "the cost of a build is not in a release");

        let steps: Vec<&str> = first.layers.iter().map(|layer| layer.step.as_str()).collect();
        assert_eq!(steps, ["test/count", "test/join", "test/upper"]);
        assert_eq!(first.layers[1].snapshots["tail"], SnapshotRead { version: "1".into(), params: Vec::new() });
        let objects: Vec<(&str, u64)> = first.objects().into_iter().collect();
        let mut expected: Vec<(&str, u64)> =
            built.iter().flat_map(|b| &b.receipt.files).map(|file| (file.sha256.as_str(), file.size)).collect();
        expected.sort();
        assert_eq!(objects, expected);

        let id = first.write(&one.store).unwrap();
        assert_eq!(hash_file(&one.store.release("test", &id)).unwrap().0, id, "the file is canonical");
        assert_eq!(Release::read(&one.store, "test", &id).unwrap(), first);

        write(&one.root().join("join.py"), &JOIN.replace("upper + tail", "tail + upper"));
        let changed = release(&one.store, &one.root(), "test", &pipeline()).unwrap();
        assert_eq!(changed, None, "a step whose layer the store lacks");
    }
}
