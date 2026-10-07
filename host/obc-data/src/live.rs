//! What live is: per product, the release that its pointer on R2 names, and the keys on R2 that
//! the live releases and their input copies use.
//!
//! A product with prefix `P` has `P/catalog.json` (the pointer: the client document,
//! `"release": "<id>"` and `"applied"`), `P/releases/<id>.json` (the manifest),
//! `P/releases/<id>/<path>` (files that a client finds by name) and `P/objects/<sha256>` (the files
//! of its client layers). The input
//! copies are `inputs/records/<source>/<version>/<digest>.json` (an exact read record) and
//! `inputs/objects/<sha256>`.

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::engine::release::{Layer, Release};
use crate::env::LiveVersions;
use crate::fetch::http::{self, Http};
use crate::input_copy::{self, Key, Record};
use crate::product::Product;
use crate::r2::{Bucket, Credentials, Object};
use crate::sources::Source;
use crate::store::{sha256_hex, write_atomic, Store};

/// The public URL of the bucket of `OBC_R2_*`.
pub const PUBLIC: &str = "https://maps.openbikecomputer.com";
/// The prefix of the input copies.
pub const INPUTS: &str = "inputs";
/// The prefix of each product, for the plumbing that runs without the products. A test of
/// `obc-data-steps` holds it equal to the prefixes of its products.
pub const PRODUCT_PREFIXES: &[&str] = &["cell-catalog", "planner"];

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
    pub(crate) fn get(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        match self {
            Remote::Bucket(bucket) => bucket.read(key),
            Remote::Public(url) => match Http::new().bytes(&format!("{url}/{key}"), "application/octet-stream") {
                Ok(bytes) => Ok(Some(bytes)),
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

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct Live {
    /// One per product, in the order of the products.
    pub products: Vec<LiveProduct>,
    /// The record that R2 holds of each snapshot that a live release reads, by source, version and input digest;
    /// `None` for a source with `r2_copy` whose record R2 lacks.
    #[serde(with = "records")]
    pub inputs: BTreeMap<Key, Option<Record>>,
}

mod records {
    use super::*;

    pub fn serialize<S: serde::Serializer>(
        value: &BTreeMap<Key, Option<Record>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        value.iter().collect::<Vec<_>>().serialize(serializer)
    }

    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<BTreeMap<Key, Option<Record>>, D::Error> {
        let rows = Vec::<(Key, Option<Record>)>::deserialize(deserializer)?;
        let count = rows.len();
        let values: BTreeMap<_, _> = rows.into_iter().collect();
        if values.len() != count {
            return Err(serde::de::Error::custom("duplicate input-copy key"));
        }
        Ok(values)
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct LiveProduct {
    pub product: String,
    pub prefix: String,
    /// The id and the manifest of the release that the pointer names. `None` without a pointer,
    /// or with a pointer that names no release.
    pub release: Option<(String, Release)>,
    /// `applied` of the pointer: the time of the switch to its release, which an apply writes.
    pub applied: Option<String>,
    /// SHA-256 of the exact observed pointer bytes, or None for a successful absent read.
    pub observed: Option<String>,
    /// The actual client document, without the publication id and time.
    pub document: Option<serde_json::Map<String, serde_json::Value>>,
}

impl Live {
    /// Read the pointer of each product, its release manifest, and the records of the input
    /// copies of `sources` that the releases read. The store keeps each manifest that it reads.
    pub fn read(remote: &Remote, products: &[&dyn Product], sources: &[Source], store: &Store) -> Result<Live, String> {
        let names: Vec<_> = products.iter().map(|product| (product.name(), product.prefix())).collect();
        Self::read_products(remote, &names, sources, store)
    }

    /// Publication reads do not link producer implementations.
    pub fn read_products(
        remote: &Remote,
        products: &[(&str, &str)],
        sources: &[Source],
        store: &Store,
    ) -> Result<Live, String> {
        let mut live = Live::default();
        for &(name, prefix) in products {
            let bytes = remote.get(&format!("{prefix}/catalog.json"))?;
            let observed = bytes.as_deref().map(sha256_hex);
            let (release, applied, document) = match pointer(bytes.as_deref(), prefix)? {
                Some((id, applied, document)) => {
                    (Some((id.clone(), manifest(remote, store, name, prefix, &id)?)), applied, Some(document))
                }
                None => (None, None, None),
            };
            live.products.push(LiveProduct {
                product: name.into(),
                prefix: prefix.into(),
                release,
                applied,
                observed,
                document,
            });
        }
        let mut records: BTreeMap<Key, Option<Record>> = BTreeMap::new();
        for read in input_copy::reads(&live)? {
            let record = match records.get(&read.key) {
                Some(record) => record.clone(),
                None => {
                    let record = input_copy::read(remote, &read.key)?;
                    records.insert(read.key.clone(), record.clone());
                    record
                }
            };
            if let Some(record) = &record {
                let names: Vec<_> = record.files.iter().map(|f| f.name.clone()).collect();
                if names != read.files {
                    return Err(format!("{}: the copy file names differ from the live read", read.key.path()));
                }
            }
            if record.is_some() || sources.iter().any(|s| s.id == read.key.source && s.r2_copy) {
                live.inputs.insert(read.key, record);
            }
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

    /// The versions of each fetch that the live layers read. Two layers can read one fetch at two
    /// versions: no order of versions chooses, so they stay for a `--move` to resolve.
    pub fn versions(&self) -> LiveVersions {
        let mut versions = LiveVersions::new();
        let layers = self.releases().flat_map(|(_, _, release)| &release.layers);
        for (source, read) in layers.flat_map(|layer| &layer.snapshots) {
            versions.entry((source.clone(), read.params.clone())).or_default().insert(read.version.clone());
        }
        versions
    }

    /// Source id to every version that the live layers read of it, in order.
    pub fn by_source(&self) -> BTreeMap<String, Vec<String>> {
        let mut sources: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (source, version) in self.snapshots() {
            sources.entry(source).or_default().push(version);
        }
        sources
    }

    /// The keys that live uses, with their size: `None` for a pointer, which changes, and for a
    /// record of an input copy.
    pub fn expected(&self) -> BTreeMap<String, Option<u64>> {
        let mut keys = BTreeMap::new();
        for (prefix, id, release) in self.releases() {
            keys.insert(format!("{prefix}/catalog.json"), None);
            keys.insert(format!("{prefix}/releases/{id}.json"), Some(release.canonical().len() as u64));
            keys.extend(
                release.named.iter().map(|file| (format!("{prefix}/releases/{id}/{}", file.path), Some(file.size))),
            );
            keys.extend(
                release.objects().into_iter().map(|(sha256, size)| (format!("{prefix}/objects/{sha256}"), Some(size))),
            );
        }
        for (key, record) in &self.inputs {
            keys.insert(key.path(), record.as_ref().map(|r| r.canonical().len() as u64));
            let files = record.iter().flat_map(|record| &record.files);
            keys.extend(files.map(|file| (format!("{INPUTS}/objects/{}", file.sha256), Some(file.size))));
        }
        keys
    }

    /// The prefixes that an apply lists: that of each product, and that of the input copies.
    pub fn prefixes(&self) -> Vec<String> {
        let prefixes = self.products.iter().map(|product| product.prefix.clone());
        prefixes.chain([INPUTS.to_string()]).collect()
    }

    /// The layers of the live releases whose files `keys` hold.
    pub fn owners(&self, keys: &[String]) -> BTreeSet<String> {
        let mut owners = BTreeSet::new();
        for (prefix, id, release) in self.releases() {
            let named: BTreeSet<_> = release
                .named
                .iter()
                .filter(|file| keys.contains(&format!("{prefix}/releases/{id}/{}", file.path)))
                .map(|file| file.sha256.as_str())
                .collect();
            let holds = |sha256: &str| keys.contains(&format!("{prefix}/objects/{sha256}"));
            let layers = release.layers.iter().filter(|layer| {
                layer.client_files().any(|file| holds(&file.sha256))
                    || layer.files.iter().any(|file| named.contains(file.sha256.as_str()))
            });
            owners.extend(layers.map(|layer| layer.step.clone()));
        }
        owners
    }

    /// Restore missing local publication metadata from its exact immutable remote key.
    pub fn restore_named(&self, remote: &Remote, store: &Store) -> Result<(), String> {
        let _using = store.using()?;
        for (prefix, id, release) in self.releases() {
            for file in &release.named {
                let key = format!("{prefix}/releases/{id}/{}", file.path);
                let object = store.object(&file.sha256);
                let _lock = store.lock(&format!("named-{}", file.sha256))?;
                if object.is_file() {
                    let (hash, size) = crate::store::hash_file(&object)?;
                    if (hash, size) != (file.sha256.clone(), file.size) {
                        return Err(format!("{key}: local named bytes have another SHA-256 or size"));
                    }
                    continue;
                }
                let Some(bytes) = remote.get(&key)? else { continue };
                if bytes.len() as u64 != file.size || sha256_hex(&bytes) != file.sha256 {
                    return Err(format!("{key}: named bytes have another SHA-256 or size"));
                }
                let part = store.partial(&format!("named-{}", file.sha256));
                write_atomic(&part, &bytes)?;
                store.insert(&part, &file.sha256)?;
            }
        }
        Ok(())
    }

    /// Every object under [`Self::prefixes`].
    pub fn list(&self, remote: &Remote) -> Result<Vec<Object>, String> {
        let listed = self.prefixes().iter().map(|prefix| remote.list(prefix)).collect::<Result<Vec<_>, _>>()?;
        Ok(listed.into_iter().flatten().collect())
    }

    /// The keys that live uses and that `listed` lacks, or holds with another size.
    pub fn drift(&self, listed: &[Object]) -> Vec<Drift> {
        let listed: BTreeMap<&str, u64> = listed.iter().map(|object| (object.key.as_str(), object.bytes)).collect();
        let drift = self.expected().into_iter().filter_map(|(key, size)| {
            let found = listed.get(key.as_str()).copied();
            (found.is_none() || size.is_some_and(|size| Some(size) != found)).then_some(Drift {
                key,
                expected: size,
                found,
            })
        });
        drift.collect()
    }

    /// The objects of `listed` that an earlier release of `obc data` used and that live does not
    /// use: the manifest of each release that is not live, its files and its input copies. A key
    /// that no manifest of `obc data` names, such as one of an older publisher, is never in it.
    /// The manifests come last, so a removal that stops part way leaves them to find the rest.
    pub fn removable(&self, remote: &Remote, store: &Store, listed: &[Object]) -> Result<Vec<Object>, String> {
        let used = self.expected();
        let live: BTreeSet<&str> = self.releases().map(|(_, id, _)| id).collect();
        let (mut files, mut manifests) = (BTreeSet::new(), BTreeSet::new());
        let mut reads = BTreeSet::new();
        for product in &self.products {
            let releases = format!("{}/releases/", product.prefix);
            let ids = listed.iter().filter_map(|object| object.key.strip_prefix(&releases)?.strip_suffix(".json"));
            for id in ids.filter(|id| !live.contains(id) && is_sha256(id)) {
                // Bytes that are not a manifest of this product belong to another publisher.
                let Ok(release) = manifest(remote, store, &product.product, &product.prefix, id) else { continue };
                manifests.insert(format!("{releases}{id}.json"));
                files.extend(release.named.iter().map(|file| format!("{releases}{id}/{}", file.path)));
                let objects = release.objects().into_keys().map(|sha256| sha256.to_string());
                files.extend(objects.map(|sha256| format!("{}/objects/{sha256}", product.prefix)));
                reads.extend(input_copy::reads_release(&release)?.into_iter().map(|read| read.key));
            }
        }
        for key in reads {
            files.insert(key.path());
            let record = input_copy::read(remote, &key)?;
            files.extend(
                record.iter().flat_map(|record| &record.files).map(|file| format!("{INPUTS}/objects/{}", file.sha256)),
            );
        }
        let listed: BTreeMap<&str, &Object> = listed.iter().map(|object| (object.key.as_str(), object)).collect();
        let removable = |keys: BTreeSet<String>| {
            let keys = keys.into_iter().filter(|key| !used.contains_key(key));
            keys.filter_map(|key| listed.get(key.as_str()).map(|object| (*object).clone())).collect::<Vec<_>>()
        };
        Ok([removable(files), removable(manifests)].concat())
    }

    /// R2 against live: what live lacks and what an apply removes.
    pub fn check(&self, remote: &Remote, store: &Store) -> Result<Check, String> {
        let listed = self.list(remote)?;
        Ok(Check {
            prefixes: self.prefixes(),
            drift: self.drift(&listed),
            leftovers: self.removable(remote, store, &listed)?,
        })
    }
}

fn is_sha256(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The prefix of a key that a removal lists it under: its first two segments.
pub fn group(key: &str) -> &str {
    key.match_indices('/').nth(1).map_or(key, |(at, _)| &key[..=at])
}

/// The count and the bytes of `removals` by [`group`].
pub fn by_prefix(removals: &[Removal]) -> BTreeMap<&str, (usize, u64)> {
    let mut groups = BTreeMap::<&str, (usize, u64)>::new();
    for removal in removals {
        let (count, bytes) = groups.entry(group(&removal.key)).or_default();
        (*count, *bytes) = (*count + 1, *bytes + removal.bytes);
    }
    groups
}

/// R2 against live.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Check {
    /// The prefixes that were listed.
    pub prefixes: Vec<String>,
    /// The keys that live uses and that R2 lacks, or holds with another size.
    pub drift: Vec<Drift>,
    /// The keys of earlier releases that no live release uses: what the next apply removes.
    pub leftovers: Vec<Object>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Drift {
    pub key: String,
    /// The size that live needs, when it is known.
    pub expected: Option<u64>,
    /// The size on R2; `None` when R2 lacks the key.
    pub found: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Removal {
    pub key: String,
    pub bytes: u64,
}

/// Refuse an older publish to `prefix` once an apply made a release live there: it would replace
/// the pointer of live, or remove what live uses.
/// A pointer that is not JSON is refused too: it can be one that an apply wrote.
pub fn refuse_older_publish(bucket: &Bucket, prefix: &str) -> Result<(), String> {
    let key = if prefix.is_empty() { "catalog.json".to_string() } else { format!("{prefix}/catalog.json") };
    let Some(bytes) = bucket.read(&key)? else { return Ok(()) };
    let pointer: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|e| format!("{key} is not JSON ({e}), so it can name a live release"))?;
    if pointer.get("release").is_some() {
        return Err(format!("{key} names a release that `obc data apply live` made live; apply live instead"));
    }
    Ok(())
}

/// Refuse a write to `keys` that only an apply makes: a key under the prefix of a product whose
/// pointer has `release`, or under `inputs` once any pointer has.
pub fn refuse_owned(bucket: &Bucket, keys: &[String]) -> Result<(), String> {
    let under = |prefix: &str| keys.iter().any(|key| key.starts_with(&format!("{prefix}/")));
    for prefix in PRODUCT_PREFIXES.iter().filter(|prefix| under(prefix) || under(INPUTS)) {
        refuse_older_publish(bucket, prefix)?;
    }
    Ok(())
}

/// The release that the pointer of `prefix` names, and its `applied`.
type ReadPointer = (String, Option<String>, serde_json::Map<String, serde_json::Value>);

fn pointer(bytes: Option<&[u8]>, prefix: &str) -> Result<Option<ReadPointer>, String> {
    let key = format!("{prefix}/catalog.json");
    let Some(bytes) = bytes else { return Ok(None) };
    let pointer: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| format!("{key}: {e}"))?;
    match pointer.get("release") {
        None => Ok(None),
        Some(serde_json::Value::String(id)) if is_sha256(id) => {
            let applied = pointer.get("applied").and_then(|applied| applied.as_str()).map(str::to_string);
            let mut document = pointer.as_object().expect("a release field belongs to an object").clone();
            document.remove("release");
            document.remove("applied");
            Ok(Some((id.clone(), applied, document)))
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
    release.check_named()?;
    Ok(release)
}

#[cfg(test)]
pub(crate) mod tests {
    use crate::engine::Client;
    use std::path::Path;

    use super::*;
    use crate::engine::release::SnapshotRead;
    use crate::engine::tests::write;
    use crate::engine::LayerFile;
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

        fn steps(
            &self,
            _root: &std::path::Path,
            _: &Env,
            _: &Regions,
            _: &Store,
        ) -> Result<crate::product::Steps, Unplanned> {
            Ok(Vec::new().into())
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
            inputs: vec![crate::engine::InputRecord {
                kind: crate::engine::InputKind::Snapshot,
                name: "land".into(),
                digest: crate::engine::digest([("land.zip", sha256_hex(b"land").as_str())]),
                files: vec!["land.zip".into()],
            }],
            options: serde_json::json!({}),
            code: String::new(),
            command: None,
            outputs: vec!["out".into()],
            digest: String::new(),
            files: vec![file],
            snapshots: [("land".to_string(), read)].into(),
            client: Client::All,
        };
        Release {
            product: "test".into(),
            region: "monaco".into(),
            optional: Vec::new(),
            layers: vec![layer],
            named: Vec::new(),
            producers: BTreeMap::new(),
        }
    }

    /// A bucket in which `release` is live, with its objects and its input copy.
    fn publish(bucket: &Path, release: &Release) {
        let id = release.id();
        write(&bucket.join("test-catalog/catalog.json"), &format!("{{\"schema\": 1, \"release\": \"{id}\"}}"));
        write(
            &bucket.join(format!("test-catalog/releases/{id}.json")),
            &String::from_utf8(release.canonical()).unwrap(),
        );
        write(&bucket.join(format!("test-catalog/objects/{}", sha256_hex(b"layer"))), "layer");
        let file = FileRecord {
            name: "land.zip".into(),
            url: "https://example.org/land.zip".into(),
            size: 4,
            sha256: sha256_hex(b"land"),
            retrieved: "2026-10-01T00:00:00Z".into(),
        };
        let record = crate::input_copy::Record {
            source: "land".into(),
            version: "2026-10-01".into(),
            files: vec![crate::input_copy::File {
                name: file.name,
                url: file.url,
                size: file.size,
                sha256: file.sha256,
            }],
        };
        let key = crate::input_copy::Key {
            source: record.source.clone(),
            version: record.version.clone(),
            digest: release.layers[0].inputs[0].digest.clone(),
        };
        write(&bucket.join(key.path()), &String::from_utf8(record.canonical()).unwrap());
        write(&bucket.join(format!("inputs/objects/{}", sha256_hex(b"land"))), "land");
    }

    #[test]
    fn a_check_finds_drift_and_the_keys_of_earlier_releases_only() {
        let scratch = Scratch::new("live-check");
        let (dir, store) = (scratch.0.join("bucket"), Store::at(scratch.0.join("store")));
        let remote = Remote::Bucket(Bucket::local(&dir));
        let sources = parse_sources(LAND).unwrap();
        let earlier = release(b"earlier");
        publish(&dir, &earlier);
        write(&dir.join(format!("test-catalog/objects/{}", sha256_hex(b"earlier"))), "earlier");
        let release = release(b"layer");
        publish(&dir, &release);
        write(&dir.join("test-catalog/objects/old"), "an older publisher");
        write(&dir.join("test-catalog/releases/old.json"), "an older publisher");
        let read = || Live::read(&remote, &[&Test], &sources, &store).unwrap();

        let live = read();
        assert_eq!(live.products[0].release, Some((release.id(), release.clone())));
        assert!(store.release("test", &release.id()).is_file(), "the store keeps the manifest");
        let check = live.check(&remote, &store).unwrap();
        assert_eq!(check.prefixes, ["test-catalog", "inputs"]);
        assert!(check.drift.is_empty(), "{check:?}");
        let leftovers: Vec<&str> = check.leftovers.iter().map(|object| object.key.as_str()).collect();
        let earlier_object = format!("test-catalog/objects/{}", sha256_hex(b"earlier"));
        let earlier_manifest = format!("test-catalog/releases/{}.json", earlier.id());
        assert_eq!(leftovers, [earlier_object.as_str(), earlier_manifest.as_str()], "the input copy is still read");

        let object = format!("inputs/objects/{}", sha256_hex(b"land"));
        std::fs::remove_file(dir.join(&object)).unwrap();
        let check = read().check(&remote, &store).unwrap();
        assert_eq!(check.drift, [Drift { key: object, expected: Some(4), found: None }]);
    }

    #[test]
    fn live_reads_the_versions_of_each_fetch() {
        let reading = |version: &str, params: &[(&str, &str)]| {
            let mut release = release(b"layer");
            let read = release.layers[0].snapshots.get_mut("land").unwrap();
            read.version = version.into();
            read.params = params.iter().map(|(name, value)| (name.to_string(), value.to_string())).collect();
            release
        };
        let live = |releases: Vec<Release>| {
            let products = releases.into_iter().map(|release| LiveProduct {
                product: "test".into(),
                prefix: "test-catalog".into(),
                release: Some((release.id(), release)),
                applied: None,
                observed: None,
                document: None,
            });
            Live { products: products.collect(), ..Live::default() }
        };
        let (a, b) = ([("area", "a")], [("area", "b")]);
        let areas = live(vec![reading("2026-10-02", &a), reading("2026-10-01", &b), reading("2026-10-02", &a)]);
        let versions: Vec<Vec<String>> =
            areas.versions().into_values().map(|read| read.into_iter().collect()).collect();
        assert_eq!(versions, [["2026-10-02"], ["2026-10-01"]], "one per params");
        assert_eq!(areas.by_source()["land"], ["2026-10-01", "2026-10-02"], "every version");

        let conflict = live(vec![reading("0.9.0", &[]), reading("0.10.2", &[])]).versions();
        assert_eq!(conflict[&("land".to_string(), Vec::new())].len(), 2, "a conflict stays");
    }

    #[test]
    fn an_older_publish_is_refused_once_a_release_is_live() {
        let scratch = Scratch::new("live-older-publish");
        let dir = scratch.0.join("bucket");
        let bucket = Bucket::local(&dir);
        assert!(refuse_older_publish(&bucket, "test-catalog").is_ok(), "no pointer");
        write(&dir.join("test-catalog/catalog.json"), "{\"schema_version\": 3}");
        assert!(refuse_older_publish(&bucket, "test-catalog").is_ok(), "the pointer of an older publish");
        publish(&dir, &release(b"layer"));
        let err = refuse_older_publish(&bucket, "test-catalog").unwrap_err();
        assert!(err.contains("apply live"), "{err}");
        write(&dir.join("test-catalog/catalog.json"), "{\"release\": ");
        assert!(refuse_older_publish(&bucket, "test-catalog").is_err(), "a pointer that is not JSON");

        let keys = |keys: &[&str]| keys.iter().map(|key| key.to_string()).collect::<Vec<_>>();
        let (copy, cell) = (keys(&["inputs/objects/a"]), keys(&["cell-catalog/objects/a"]));
        assert!(refuse_owned(&bucket, &copy).is_ok() && refuse_owned(&bucket, &cell).is_ok(), "nothing live");
        write(&dir.join("cell-catalog/catalog.json"), &format!("{{\"release\": \"{}\"}}", "a".repeat(64)));
        assert!(refuse_owned(&bucket, &cell).is_err() && refuse_owned(&bucket, &copy).is_err());
        assert!(refuse_owned(&bucket, &keys(&["planner/objects/a", "firmware/v1/app.bin"])).is_ok());

        let reference = keys(&["reference/v1/index.json", "reference/v1/16/1.tif"]);
        assert!(refuse_owned(&bucket, &reference).is_ok(), "the older reference publisher remains available");
    }
    #[test]
    fn a_record_on_r2_counts_without_r2_copy() {
        let scratch = Scratch::new("live-record");
        let (dir, store) = (scratch.0.join("bucket"), Store::at(scratch.0.join("store")));
        let remote = Remote::Bucket(Bucket::local(&dir));
        publish(&dir, &release(b"layer"));
        let live = Live::read(&remote, &[&Test], &[], &store).unwrap();
        assert_eq!(live.inputs.len(), 1);
        let check = live.check(&remote, &store).unwrap();
        assert!(check.drift.is_empty() && check.leftovers.is_empty(), "{check:?}");
    }

    /// A product whose pointer an older publish wrote, without `release`.
    struct Old;

    impl Product for Old {
        fn name(&self) -> &'static str {
            "old"
        }

        fn steps(
            &self,
            _root: &std::path::Path,
            _: &Env,
            _: &Regions,
            _: &Store,
        ) -> Result<crate::product::Steps, Unplanned> {
            Ok(Vec::new().into())
        }
    }

    #[test]
    fn a_product_with_nothing_live_has_no_leftovers() {
        let scratch = Scratch::new("live-old");
        let (dir, store) = (scratch.0.join("bucket"), Store::at(scratch.0.join("store")));
        let remote = Remote::Bucket(Bucket::local(&dir));
        let sources = parse_sources(LAND).unwrap();
        publish(&dir, &release(b"layer"));
        write(&dir.join("old/catalog.json"), "{\"schema_version\": 3}");
        write(&dir.join("old/cells/a.obcm"), "cell");

        let live = Live::read(&remote, &[&Test, &Old], &sources, &store).unwrap();
        assert!(live.products[1].release.is_none(), "a pointer without `release` names nothing");
        let check = live.check(&remote, &store).unwrap();
        assert_eq!(check.prefixes, ["test-catalog", "old", "inputs"]);
        assert!(check.drift.is_empty() && check.leftovers.is_empty(), "{check:?}");
    }

    #[test]
    fn removals_group_by_the_first_two_segments_of_their_key() {
        let removal = |key: &str, bytes| Removal { key: key.into(), bytes };
        let removals = [removal("cell-catalog/objects/a", 2), removal("cell-catalog/objects/b", 3), removal("x", 1)];
        assert_eq!(
            by_prefix(&removals).into_iter().collect::<Vec<_>>(),
            [("cell-catalog/objects/", (2, 5)), ("x", (1, 1))]
        );
    }
}
