//! What live is: per product, the release that its pointer on R2 names, and the keys on R2 that
//! the live releases and their input copies use.
//!
//! A product with prefix `P` has `P/catalog.json` (the pointer: the client document and
//! `"release": "<id>"`), `P/releases/<id>.json` (the manifest), `P/releases/<id>/<path>` (files
//! that a client finds by name) and `P/objects/<sha256>` (the files of its layers). The input
//! copies are `inputs/records/<source>/<version>.json` (a snapshot record) and
//! `inputs/objects/<sha256>`.

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::Serialize;

use crate::engine::release::{Layer, Release};
use crate::fetch::http::{self, Http};
use crate::product::Product;
use crate::r2::{Bucket, Credentials, Object};
use crate::sources::Source;
use crate::store::{sha256_hex, write_atomic, Snapshot, Store};

/// The public URL of the bucket of `OBC_R2_*`.
pub const PUBLIC: &str = "https://maps.openbikecomputer.com";
/// The prefix of the input copies.
pub const INPUTS: &str = "inputs";

/// Where live is read: the bucket of `OBC_R2_*` when `OBC_R2_BUCKET` or `OBC_R2_LOCAL_DIR` is
/// set, or else its public URL.
pub enum Remote {
    Bucket(Bucket),
    Public(String),
}

impl Remote {
    pub fn from_env() -> Result<Self, String> {
        let bucket = std::env::var_os("OBC_R2_BUCKET").is_some_and(|bucket| !bucket.is_empty());
        if bucket || std::env::var_os("OBC_R2_LOCAL_DIR").is_some() {
            return Bucket::from_env(Credentials::Main).map(Remote::Bucket);
        }
        Ok(Remote::Public(PUBLIC.into()))
    }

    pub fn describe(&self) -> &str {
        match self {
            Remote::Bucket(bucket) => bucket.describe(),
            Remote::Public(url) => url,
        }
    }

    /// The bytes of `key`, or `None` when there is no such object.
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        match self {
            Remote::Bucket(bucket) => bucket.read(key),
            Remote::Public(url) => match Http::new().text(&format!("{url}/{key}"), "application/json") {
                Ok(text) => Ok(Some(text.into_bytes())),
                Err(e) if http::not_found(&e) => Ok(None),
                Err(e) => Err(e),
            },
        }
    }

    /// Every object under the folder `prefix`. A listing needs the credential.
    pub fn list(&self, prefix: &str) -> Result<Vec<Object>, String> {
        match self {
            Remote::Bucket(bucket) => bucket.list(prefix),
            Remote::Public(_) => Err("a listing of R2 needs the OBC_R2_* variables".into()),
        }
    }
}

#[derive(Debug, Default)]
pub struct Live {
    /// One per product, in the order of the products.
    pub products: Vec<LiveProduct>,
    /// The record of each input copy that a live release reads, by source and version; `None`
    /// when R2 does not have it.
    pub inputs: BTreeMap<(String, String), Option<Snapshot>>,
}

#[derive(Debug)]
pub struct LiveProduct {
    pub product: String,
    pub prefix: String,
    /// The id and the manifest of the release that the pointer names. `None` without a pointer,
    /// or with a pointer that names no release.
    pub release: Option<(String, Release)>,
}

impl Live {
    /// Read the pointer of each product, its release manifest, and the records of the input
    /// copies of `sources` that the releases read. The store keeps each manifest that it reads.
    pub fn read(remote: &Remote, products: &[&dyn Product], sources: &[Source], store: &Store) -> Result<Live, String> {
        let mut live = Live::default();
        for product in products {
            let prefix = product.prefix();
            let release = match pointer(remote, prefix)? {
                Some(id) => Some((id.clone(), manifest(remote, store, product.name(), prefix, &id)?)),
                None => None,
            };
            live.products.push(LiveProduct { product: product.name().into(), prefix: prefix.into(), release });
        }
        for (source, version) in live.snapshots() {
            if !sources.iter().any(|s| s.id == source && s.r2_copy) {
                continue;
            }
            let key = record_key(&source, &version);
            let record = remote.get(&key)?.map(|bytes| serde_json::from_slice(&bytes)).transpose();
            live.inputs.insert((source, version), record.map_err(|e| format!("{key}: {e}"))?);
        }
        Ok(live)
    }

    /// The id and the manifest of each live release, with its prefix.
    pub fn releases(&self) -> impl Iterator<Item = (&str, &str, &Release)> {
        let releases = self.products.iter().filter_map(|p| p.release.as_ref().map(|(id, r)| (p, id, r)));
        releases.map(|(product, id, release)| (product.prefix.as_str(), id.as_str(), release))
    }

    /// Each layer of the live releases, by step.
    pub fn layers(&self) -> BTreeMap<String, Layer> {
        let layers = self.releases().flat_map(|(_, _, release)| &release.layers);
        layers.map(|layer| (layer.step.clone(), layer.clone())).collect()
    }

    /// The source and version of each snapshot that a live layer read.
    pub fn snapshots(&self) -> BTreeSet<(String, String)> {
        let layers = self.releases().flat_map(|(_, _, release)| &release.layers);
        let reads = layers.flat_map(|layer| &layer.snapshots);
        reads.map(|(source, read)| (source.clone(), read.version.clone())).collect()
    }

    /// Source id to the version that the live layers read: the newest, when two layers read two.
    pub fn versions(&self) -> BTreeMap<String, String> {
        // The set is in order, so the newest version of a source comes last and stays.
        self.snapshots().into_iter().collect()
    }

    /// The keys that live uses, with their size: `None` for a pointer, which changes, and for a
    /// record of an input copy.
    pub fn expected(&self) -> BTreeMap<String, Option<u64>> {
        let mut keys = BTreeMap::new();
        for (prefix, id, release) in self.releases() {
            keys.insert(format!("{prefix}/catalog.json"), None);
            keys.insert(format!("{prefix}/releases/{id}.json"), Some(release.canonical().len() as u64));
            keys.extend(
                release.objects().into_iter().map(|(sha256, size)| (format!("{prefix}/objects/{sha256}"), Some(size))),
            );
        }
        for ((source, version), record) in &self.inputs {
            keys.insert(record_key(source, version), None);
            let files = record.iter().flat_map(|record| &record.files);
            keys.extend(files.map(|file| (format!("{INPUTS}/objects/{}", file.sha256), Some(file.size))));
        }
        keys
    }

    /// The prefixes that live owns: that of each product with a live release, and that of the
    /// input copies once a release is live. A product with nothing live owns nothing, so what an
    /// older publish left there is never a leftover.
    pub fn prefixes(&self) -> Vec<String> {
        let mut prefixes: Vec<String> = self.releases().map(|(prefix, _, _)| prefix.to_string()).collect();
        if !prefixes.is_empty() {
            prefixes.push(INPUTS.into());
        }
        prefixes
    }

    /// What a listing of the owned prefixes shows against live.
    pub fn check(&self, remote: &Remote) -> Result<Check, String> {
        let mut listed = BTreeMap::new();
        for prefix in self.prefixes() {
            listed.extend(remote.list(&prefix)?.into_iter().map(|object| (object.key.clone(), object)));
        }
        let expected = self.expected();
        let drift = expected.iter().filter_map(|(key, &size)| {
            let found = listed.get(key).map(|object| object.bytes);
            (found.is_none() || size.is_some_and(|size| Some(size) != found)).then(|| Drift {
                key: key.clone(),
                expected: size,
                found,
            })
        });
        let drift = drift.collect();
        let named: Vec<String> = self.releases().map(|(prefix, id, _)| format!("{prefix}/releases/{id}/")).collect();
        let used = |key: &str| expected.contains_key(key) || named.iter().any(|n| key.starts_with(n));
        let leftovers = listed.into_values().filter(|object| !used(&object.key));
        Ok(Check { prefixes: self.prefixes(), drift, leftovers: leftovers.collect() })
    }
}

/// The owned prefixes of R2 against live.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Check {
    /// The prefixes that were listed.
    pub prefixes: Vec<String>,
    /// The keys that live uses and that R2 lacks, or holds with another size.
    pub drift: Vec<Drift>,
    /// The keys under the prefixes that no live release uses.
    pub leftovers: Vec<Object>,
}

#[derive(Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Drift {
    pub key: String,
    /// The size that live needs, when it is known.
    pub expected: Option<u64>,
    /// The size on R2; `None` when R2 lacks the key.
    pub found: Option<u64>,
}

fn record_key(source: &str, version: &str) -> String {
    format!("{INPUTS}/records/{source}/{version}.json")
}

/// The release that the pointer of `prefix` names.
fn pointer(remote: &Remote, prefix: &str) -> Result<Option<String>, String> {
    let key = format!("{prefix}/catalog.json");
    let Some(bytes) = remote.get(&key)? else { return Ok(None) };
    let pointer: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| format!("{key}: {e}"))?;
    match pointer.get("release") {
        None => Ok(None),
        Some(serde_json::Value::String(id))
            if id.len() == 64 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) =>
        {
            Ok(Some(id.clone()))
        }
        Some(other) => Err(format!("{key}: `release` is {other}, not the SHA-256 of a manifest")),
    }
}

/// The manifest of release `id`: from the store when its copy there is that manifest, or else
/// from R2 into the store.
fn manifest(remote: &Remote, store: &Store, product: &str, prefix: &str, id: &str) -> Result<Release, String> {
    let path = store.release(product, id);
    if let Some(release) = std::fs::read(&path).ok().and_then(|bytes| verified(&bytes, product, id, "").ok()) {
        return Ok(release);
    }
    let key = format!("{prefix}/releases/{id}.json");
    let bytes =
        remote.get(&key)?.ok_or_else(|| format!("{prefix}/catalog.json names release {id}, but {key} is missing"))?;
    let release = verified(&bytes, product, id, &key)?;
    write_atomic(&path, &bytes)?;
    Ok(release)
}

/// The manifest in `bytes`, the file `name`, when its SHA-256 is `id` and it is of `product`.
fn verified(bytes: &[u8], product: &str, id: &str, name: &str) -> Result<Release, String> {
    if sha256_hex(bytes) != id {
        return Err(format!("{name}: its SHA-256 is not its id"));
    }
    let release: Release = serde_json::from_slice(bytes).map_err(|e| format!("{name}: {e}"))?;
    if release.product != product {
        return Err(format!("{name}: the manifest is of product `{}`, not `{product}`", release.product));
    }
    Ok(release)
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::Path;

    use super::*;
    use crate::engine::release::SnapshotRead;
    use crate::engine::tests::write;
    use crate::engine::{LayerFile, Step};
    use crate::env::Env;
    use crate::product::Unplanned;
    use crate::regions::Regions;
    use crate::sources::parse_sources;
    use crate::store::tests::Scratch;
    use crate::store::FileRecord;

    pub(crate) struct Test;

    impl Product for Test {
        fn name(&self) -> &'static str {
            "test"
        }

        fn prefix(&self) -> &'static str {
            "test-catalog"
        }

        fn steps(&self, _: &Env, _: &Regions, _: &Store) -> Result<Vec<Step>, Unplanned> {
            Ok(Vec::new())
        }
    }

    pub(crate) const LAND: &str = r#"
        [[source]]
        id = "land"
        kind = "data"
        licence = "ODbL-1.0"
        fetch = { kind = "http", url = "https://example.org/land.zip" }
        version = "date"
        refresh = 30
        redistribute = true
        r2_copy = true
    "#;

    /// A release of one layer with the file `bytes`, which read `land@2026-10-01`.
    pub(crate) fn release(bytes: &[u8]) -> Release {
        let file = LayerFile { path: "out".into(), size: bytes.len() as u64, sha256: sha256_hex(bytes) };
        let read = SnapshotRead { version: "2026-10-01".into(), params: Vec::new() };
        let layer = Layer {
            step: "test/one".into(),
            key: "key".into(),
            inputs: Vec::new(),
            options: serde_json::json!({}),
            code: String::new(),
            command: None,
            outputs: vec!["out".into()],
            digest: String::new(),
            files: vec![file],
            snapshots: [("land".to_string(), read)].into(),
        };
        Release { product: "test".into(), layers: vec![layer] }
    }

    /// A bucket in which `release` is live, with its objects and its input copy.
    fn publish(bucket: &Path, release: &Release) {
        let id = release.id();
        write(&bucket.join("test-catalog/catalog.json"), &format!("{{\"schema\": 1, \"release\": \"{id}\"}}"));
        write(
            &bucket.join(format!("test-catalog/releases/{id}.json")),
            &String::from_utf8(release.canonical()).unwrap(),
        );
        write(&bucket.join(format!("test-catalog/releases/{id}/LICENSE.txt")), "named");
        write(&bucket.join(format!("test-catalog/objects/{}", sha256_hex(b"layer"))), "layer");
        let file = FileRecord {
            name: "land.zip".into(),
            url: "https://example.org/land.zip".into(),
            size: 4,
            sha256: sha256_hex(b"land"),
            retrieved: "2026-10-01T00:00:00Z".into(),
        };
        let record = Snapshot { source: "land".into(), version: "2026-10-01".into(), files: vec![file] };
        write(&bucket.join("inputs/records/land/2026-10-01.json"), &serde_json::to_string(&record).unwrap());
        write(&bucket.join(format!("inputs/objects/{}", sha256_hex(b"land"))), "land");
    }

    #[test]
    fn a_check_finds_drift_and_leftovers_against_the_live_release() {
        let scratch = Scratch::new("live-check");
        let (dir, store) = (scratch.0.join("bucket"), Store::at(scratch.0.join("store")));
        let remote = Remote::Bucket(Bucket::local(&dir));
        let sources = parse_sources(LAND).unwrap();
        let release = release(b"layer");
        publish(&dir, &release);
        let read = || Live::read(&remote, &[&Test], &sources, &store).unwrap();

        let live = read();
        assert_eq!(live.products[0].release, Some((release.id(), release.clone())));
        assert!(store.release("test", &release.id()).is_file(), "the store keeps the manifest");
        let check = live.check(&remote).unwrap();
        assert_eq!(check.prefixes, ["test-catalog", "inputs"]);
        assert_eq!((check.drift.len(), check.leftovers.len()), (0, 0), "{check:?}");

        let object = format!("inputs/objects/{}", sha256_hex(b"land"));
        std::fs::remove_file(dir.join(&object)).unwrap();
        write(&dir.join("test-catalog/objects/old"), "old");
        let check = read().check(&remote).unwrap();
        assert_eq!(check.drift, [Drift { key: object, expected: Some(4), found: None }]);
        let leftovers: Vec<&str> = check.leftovers.iter().map(|object| object.key.as_str()).collect();
        assert_eq!(leftovers, ["test-catalog/objects/old"]);
    }

    /// A product whose pointer an older publish wrote, without `release`.
    struct Old;

    impl Product for Old {
        fn name(&self) -> &'static str {
            "old"
        }

        fn steps(&self, _: &Env, _: &Regions, _: &Store) -> Result<Vec<Step>, Unplanned> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn a_product_with_nothing_live_owns_no_prefix() {
        let scratch = Scratch::new("live-old");
        let (dir, store) = (scratch.0.join("bucket"), Store::at(scratch.0.join("store")));
        let remote = Remote::Bucket(Bucket::local(&dir));
        let sources = parse_sources(LAND).unwrap();
        publish(&dir, &release(b"layer"));
        write(&dir.join("old/catalog.json"), "{\"schema_version\": 3}");
        write(&dir.join("old/cells/a.obcm"), "cell");

        let live = Live::read(&remote, &[&Test, &Old], &sources, &store).unwrap();
        assert!(live.products[1].release.is_none(), "a pointer without `release` names nothing");
        let check = live.check(&remote).unwrap();
        assert_eq!(check.prefixes, ["test-catalog", "inputs"], "only a live product owns its prefix");
        assert!(check.drift.is_empty() && check.leftovers.is_empty(), "{check:?}");

        write(&dir.join("test-catalog/catalog.json"), "{\"schema_version\": 3}");
        let live = Live::read(&remote, &[&Test, &Old], &sources, &store).unwrap();
        assert!(live.inputs.is_empty());
        let check = live.check(&remote).unwrap();
        assert!(check.prefixes.is_empty() && check.leftovers.is_empty(), "nothing live owns nothing: {check:?}");
    }
}
