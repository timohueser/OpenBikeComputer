//! Immutable records of the exact input files that a live layer reads. Restoration shares the
//! upstream fetch boundary and downloads only the requested objects.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::engine::{digest, InputKind};
use crate::fetch::{
    self,
    http::{Expect, Http},
    Request,
};
use crate::live::{Live, Remote, INPUTS};
use crate::store::{hash_file, sorted, FileRecord, Requested, Snapshot, Store};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Key {
    pub source: String,
    pub version: String,
    pub digest: String,
}

impl Key {
    pub fn path(&self) -> String {
        format!("{INPUTS}/records/{}/{}/{}.json", self.source, self.version, self.digest)
    }
}

#[derive(Debug, Clone)]
pub struct Read {
    pub step: String,
    pub key: Key,
    pub params: Vec<(String, String)>,
    pub files: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Retained {
    pub step: String,
    pub key: Key,
    pub params: Vec<(String, String)>,
    pub record: Record,
}

pub fn retained(live: &Live, store: &Store) -> Result<Vec<Retained>, String> {
    let mut retained = Vec::new();
    for read in reads(live)? {
        if let Some(remote) = live.inputs.get(&read.key) {
            let record = match remote {
                Some(record) => Some(record.clone()),
                None => Record::local(store, &read)?,
            };
            if let Some(record) = record {
                retained.push(Retained { step: read.step, key: read.key, params: read.params, record });
            }
        }
    }
    Ok(retained)
}

/// Retrieval time is local provenance. It does not change an immutable copy record.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct File {
    pub name: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub source: String,
    pub version: String,
    pub files: Vec<File>,
}

impl Record {
    pub fn local(store: &Store, read: &Read) -> Result<Option<Self>, String> {
        if read.files.is_empty() && read.key.digest != digest([]) {
            return Err(format!(
                "{}: the receipt lacks exact input file names; resolve its inputs first",
                read.key.path()
            ));
        }
        let snapshot = store.snapshot(&read.key.source, &read.key.version)?;
        if snapshot.as_ref().is_some_and(|s| s.source != read.key.source || s.version != read.key.version) {
            return Err(format!("{}: the local snapshot has another source or version", read.key.path()));
        }
        if read.files.is_empty() {
            let known = if read.params.is_empty() {
                snapshot.as_ref().is_some_and(|s| s.files.is_empty())
            } else {
                store
                    .requested(&read.key.source, &read.key.version, &read.params)?
                    .is_some_and(|files| files.is_empty())
            };
            if !known {
                return Ok(None);
            }
        }
        let mut files = Vec::new();
        for name in &read.files {
            let Some(file) = snapshot.as_ref().and_then(|s| s.files.iter().find(|f| &f.name == name)) else {
                return Ok(None);
            };
            files.push(File {
                name: file.name.clone(),
                url: file.url.clone(),
                size: file.size,
                sha256: file.sha256.clone(),
            });
        }
        files.sort_by(|a, b| a.name.cmp(&b.name));
        let record = Self { source: read.key.source.clone(), version: read.key.version.clone(), files };
        record.validate(&read.key)?;
        Ok(Some(record))
    }

    pub fn validate(&self, key: &Key) -> Result<(), String> {
        let mut names = BTreeSet::new();
        let mut urls = BTreeSet::new();
        for file in &self.files {
            let relative =
                !file.name.contains('\\') && file.name.split('/').all(|s| !s.is_empty() && s != "." && s != "..");
            if !relative || !names.insert(&file.name) || !urls.insert(&file.url) || !sha256(&file.sha256) {
                return Err(format!("{}: invalid or duplicate input file {}", key.path(), file.name));
            }
        }
        let actual = digest(self.files.iter().map(|f| (f.name.as_str(), f.sha256.as_str())));
        if self.source != key.source || self.version != key.version || actual != key.digest {
            return Err(format!("{}: the input copy has another source, version or digest", key.path()));
        }
        Ok(())
    }

    pub(crate) fn check_local(&self, store: &Store) -> Result<(), String> {
        let _lock = store.lock(&fetch::snapshot_lock(&self.source, &self.version))?;
        fetch::merge(store, &self.source, &self.version, &self.snapshot().files).map(drop)
    }

    fn snapshot(&self) -> Snapshot {
        let retrieved = crate::date::timestamp(crate::date::now());
        Snapshot {
            source: self.source.clone(),
            version: self.version.clone(),
            files: self
                .files
                .iter()
                .map(|f| FileRecord {
                    name: f.name.clone(),
                    url: f.url.clone(),
                    size: f.size,
                    sha256: f.sha256.clone(),
                    retrieved: retrieved.clone(),
                })
                .collect(),
        }
    }

    pub fn canonical(&self) -> Vec<u8> {
        let value = serde_json::to_value(self).expect("an input copy serializes");
        serde_json::to_vec(&crate::engine::sorted(value)).expect("JSON values serialize")
    }
}

fn sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The reads named by the manifests. No input object or copy record is fetched here.
pub fn reads(live: &Live) -> Result<Vec<Read>, String> {
    let mut reads = Vec::new();
    for layer in live.releases().flat_map(|(_, _, r)| &r.layers) {
        for input in layer.inputs.iter().filter(|i| i.kind == InputKind::Snapshot) {
            let snapshot = layer
                .snapshots
                .get(&input.name)
                .ok_or_else(|| format!("{}: no snapshot version of {}", layer.step, input.name))?;
            if !crate::is_kebab(&input.name)
                || snapshot.version.is_empty()
                || snapshot.version.contains(['/', '\\'])
                || snapshot.version == "."
                || snapshot.version == ".."
                || !sha256(&input.digest)
            {
                return Err(format!("{}: invalid snapshot identity", layer.step));
            }
            reads.push(Read {
                step: layer.step.clone(),
                key: Key {
                    source: input.name.clone(),
                    version: snapshot.version.clone(),
                    digest: input.digest.clone(),
                },
                params: sorted(&snapshot.params),
                files: input.files.clone(),
            });
        }
    }
    Ok(reads)
}

pub fn read(remote: &Remote, key: &Key) -> Result<Option<Record>, String> {
    let record: Option<Record> = remote
        .get(&key.path())?
        .map(|bytes| serde_json::from_slice(&bytes))
        .transpose()
        .map_err(|e| format!("{}: {e}", key.path()))?;
    if let Some(record) = &record {
        record.validate(key)?;
    }
    Ok(record)
}

/// One optional context for CLI discovery and run fetches. Status applies its source allowlist
/// before this boundary, so opening status never restores bulk inputs.
pub struct Restore<'a> {
    pub remote: &'a Remote,
    pub live: &'a Live,
}

impl Restore<'_> {
    fn matching(&self, request: &Request) -> Result<Vec<Read>, String> {
        let Some(version) = &request.version else { return Ok(Vec::new()) };
        let params = sorted(&request.params);
        let reads: Vec<_> = reads(self.live)?
            .into_iter()
            .filter(|r| r.key.source == request.source.id && r.key.version == *version && r.params == params)
            .filter(|r| self.live.inputs.contains_key(&r.key))
            .collect();
        if !params.is_empty() && reads.iter().map(|read| &read.key.digest).collect::<BTreeSet<_>>().len() > 1 {
            return Err(format!(
                "{}@{version}: live has conflicting retained files for the same request",
                request.source.id
            ));
        }
        Ok(reads)
    }

    fn local(&self, store: &Store, request: &Request, selected: &[String]) -> Result<Option<Snapshot>, String> {
        let reads = self.matching(request)?;
        if reads.is_empty() {
            return Ok(None);
        }
        let mut files = BTreeMap::new();
        for read in &reads {
            if let Some(Some(record)) = self.live.inputs.get(&read.key) {
                record.check_local(store)?;
            }
            let Some(record) = Record::local(store, read)? else { return Ok(None) };
            for file in record.files {
                files.insert(file.name.clone(), file);
            }
        }
        if selected.iter().any(|name| !files.contains_key(name)) {
            return Ok(None);
        }
        let snapshot = store.snapshot(&request.source.id, request.version.as_deref().expect("matched version"))?;
        let mut selected_files = Vec::new();
        for file in files.values().filter(|file| selected.is_empty() || selected.contains(&file.name)) {
            let object = store.object(&file.sha256);
            if !object.is_file() {
                return Ok(None);
            }
            let (hash, size) = hash_file(&object)?;
            if hash != file.sha256 || size != file.size {
                return Err(format!("{}: the local retained object has another SHA-256 or size", object.display()));
            }
            selected_files.push(
                snapshot
                    .as_ref()
                    .expect("local record has files")
                    .files
                    .iter()
                    .find(|f| f.name == file.name)
                    .expect("resolved file")
                    .clone(),
            );
        }
        if !request.params.is_empty() {
            store.put_requested(
                &request.source.id,
                &Requested {
                    version: request.version.clone().expect("matched version"),
                    params: sorted(&request.params),
                    files: files.into_keys().collect(),
                },
            )?;
        }
        Ok(Some(Snapshot {
            source: request.source.id.clone(),
            version: request.version.clone().expect("matched version"),
            files: selected_files,
        }))
    }

    fn restore(
        &self,
        store: &Store,
        http: &Http,
        request: &Request,
        selected: &[String],
    ) -> Result<Option<Snapshot>, String> {
        let reads = self.matching(request)?;
        if reads.is_empty() {
            return Ok(None);
        }
        let version = request.version.as_ref().expect("matched version");
        let params = sorted(&request.params);
        let mut records = BTreeMap::new();
        for read in reads {
            records.entry(read.key.clone()).or_insert(read);
        }
        let mut files = BTreeMap::new();
        for (key, wanted) in records {
            let record = self
                .live
                .inputs
                .get(&key)
                .cloned()
                .flatten()
                .ok_or_else(|| format!("{}: the retained input copy is missing", key.path()))?;
            let names: Vec<_> = record.files.iter().map(|f| f.name.clone()).collect();
            if names != wanted.files {
                return Err(format!("{}: the copy file names differ from the live read", key.path()));
            }
            for file in record.files {
                let record = FileRecord {
                    name: file.name.clone(),
                    url: file.url,
                    size: file.size,
                    sha256: file.sha256,
                    retrieved: crate::date::timestamp(crate::date::now()),
                };
                if files.get(&file.name).is_some_and(|old: &FileRecord| {
                    old.url != record.url || old.sha256 != record.sha256 || old.size != record.size
                }) {
                    return Err(format!("{}@{version}: conflicting retained file {}", request.source.id, file.name));
                }
                files.insert(file.name, record);
            }
        }
        if selected.iter().any(|name| !files.contains_key(name)) {
            return Ok(None);
        }
        let all: Vec<_> = files.into_values().collect();
        let _using = store.using()?;
        let _lock = store.lock(&fetch::snapshot_lock(&request.source.id, version))?;
        let merged = fetch::merge(store, &request.source.id, version, &all)?;
        for file in all.iter().filter(|file| selected.is_empty() || selected.contains(&file.name)) {
            let object = store.object(&file.sha256);
            if !object.is_file() {
                let key = format!("{INPUTS}/objects/{}", file.sha256);
                let _object = store.lock(&format!("copy-object-{}", file.sha256))?;
                match self.remote {
                    Remote::Bucket(bucket) => {
                        let part = store.partial(&format!("copy-{}.part", file.sha256));
                        std::fs::create_dir_all(part.parent().expect("a partial has a parent"))
                            .map_err(|e| e.to_string())?;
                        bucket.get(&key, &part)?;
                        let (hash, size) = hash_file(&part)?;
                        if hash != file.sha256 || size != file.size {
                            let _ = std::fs::remove_file(&part);
                            return Err(format!("{key}: the retained bytes have another SHA-256 or size"));
                        }
                        store.insert(&part, &file.sha256)?;
                    }
                    Remote::Public(url) => {
                        let url = format!("{url}/{key}");
                        let _lock = Http::lock(store, &url)?;
                        http.download(store, &url, &Expect { sha256: Some(&file.sha256), ..Expect::default() })?;
                    }
                }
            }
            let (hash, size) = hash_file(&object)?;
            if hash != file.sha256 || size != file.size {
                return Err(format!("{}: the retained object has another SHA-256 or size", object.display()));
            }
        }
        if let Some(snapshot) = merged {
            store.put_snapshot(&snapshot)?;
        }
        if !params.is_empty() {
            store.put_requested(
                &request.source.id,
                &Requested { version: version.clone(), params, files: all.iter().map(|f| f.name.clone()).collect() },
            )?;
        }
        Ok(Some(Snapshot {
            source: request.source.id.clone(),
            version: version.clone(),
            files: all.into_iter().filter(|f| selected.is_empty() || selected.contains(&f.name)).collect(),
        }))
    }
}

pub fn fetch(
    store: &Store,
    http: &Http,
    copies: Option<&Restore>,
    request: &Request,
    selected: &[String],
) -> Result<Snapshot, String> {
    if let Some(snapshot) = copies
        .map(|c| {
            c.local(store, request, selected).and_then(|local| match local {
                Some(snapshot) => Ok(Some(snapshot)),
                None => c.restore(store, http, request, selected),
            })
        })
        .transpose()?
        .flatten()
    {
        return Ok(snapshot);
    }
    fetch::fetch(store, http, request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::tests::{fixture, write};
    use crate::engine::{release::SnapshotRead, InputRecord};
    use crate::live::LiveProduct;
    use crate::r2::Bucket;
    use crate::store::sha256_hex;

    type RequestRead<'a> = (&'a str, &'a str, &'a [&'a str], Vec<(String, String)>);

    fn live(store: &Store, requests: &[RequestRead<'_>]) -> Live {
        let mut release = crate::live::tests::release(b"layer");
        release.layers.clear();
        for (n, (source, version, names, params)) in requests.iter().enumerate() {
            let mut layer = crate::live::tests::release(b"layer").layers.remove(0);
            let snapshot = store.snapshot(source, version).unwrap().unwrap();
            let files: Vec<_> =
                names.iter().map(|name| snapshot.files.iter().find(|f| &f.name == name).unwrap()).collect();
            layer.step = format!("test/{n}");
            layer.inputs = vec![InputRecord {
                kind: InputKind::Snapshot,
                name: source.to_string(),
                digest: digest(files.iter().map(|f| (f.name.as_str(), f.sha256.as_str()))),
                files: names.iter().map(|n| n.to_string()).collect(),
            }];
            layer.snapshots = BTreeMap::from([(
                source.to_string(),
                SnapshotRead { version: version.to_string(), params: sorted(params) },
            )]);
            release.layers.push(layer);
        }
        Live {
            products: vec![LiveProduct {
                product: "test".into(),
                prefix: "test".into(),
                release: Some((release.id(), release)),
                applied: None,
                observed: None,
                document: None,
            }],
            ..Live::default()
        }
    }

    fn copy(store: &Store, dir: &std::path::Path, live: &mut Live) {
        for read in reads(live).unwrap() {
            let record = Record::local(store, &read).unwrap().unwrap();
            write(&dir.join(read.key.path()), &String::from_utf8(record.canonical()).unwrap());
            for file in &record.files {
                let to = dir.join(format!("{INPUTS}/objects/{}", file.sha256));
                std::fs::create_dir_all(to.parent().unwrap()).unwrap();
                std::fs::copy(store.object(&file.sha256), to).unwrap();
            }
            live.inputs.insert(read.key, Some(record));
        }
    }

    #[test]
    fn exact_immutable_reads_stage_independently_and_restore_only_the_selected_region() {
        let fixture = fixture("copy-reads");
        for (name, bytes) in [("small.bin", b"small".as_slice()), ("huge.bin", b"huge"), ("unused.bin", b"unused")] {
            fixture.fetched_version("land", "2026-10-01", name, bytes);
        }
        let small = vec![("area".into(), "small".into())];
        let huge = vec![("area".into(), "huge".into())];
        let mut old = live(&fixture.store, &[("land", "2026-10-01", &["small.bin"], small.clone())]);
        let dir = fixture.scratch.0.join("bucket");
        copy(&fixture.store, &dir, &mut old);
        let old_key = old.inputs.keys().next().unwrap().path();
        let old_bytes = std::fs::read(dir.join(&old_key)).unwrap();
        let mut staged = live(&fixture.store, &[("land", "2026-10-01", &["huge.bin"], huge)]);
        copy(&fixture.store, &dir, &mut staged);
        assert_eq!(
            std::fs::read(dir.join(&old_key)).unwrap(),
            old_bytes,
            "staging a different read never narrows the old record"
        );
        old.products[0]
            .release
            .as_mut()
            .unwrap()
            .1
            .layers
            .extend(staged.products[0].release.as_ref().unwrap().1.layers.clone());
        old.inputs.extend(staged.inputs);
        let fresh = Store::at(fixture.scratch.0.join("fresh"));
        let source = crate::sources::parse_sources(crate::live::tests::LAND).unwrap().remove(0);
        let remote = Remote::Bucket(Bucket::local(&dir));
        let restored = fetch(
            &fresh,
            &Http::new(),
            Some(&Restore { remote: &remote, live: &old }),
            &Request { source: &source, version: Some("2026-10-01".into()), params: small.clone() },
            &[],
        )
        .unwrap();
        assert_eq!(restored.files.iter().map(|f| &f.name).collect::<Vec<_>>(), [&"small.bin".to_string()]);
        assert_eq!(fresh.requested("land", "2026-10-01", &small).unwrap(), Some(vec!["small.bin".into()]));
        assert!(!fresh.object(&sha256_hex(b"huge")).exists());
        assert!(!fresh.object(&sha256_hex(b"unused")).exists());
        assert!(!old.expected().contains_key(&format!("{INPUTS}/objects/{}", sha256_hex(b"unused"))));
        old.products[0].release.as_mut().unwrap().1.layers[1].snapshots.get_mut("land").unwrap().params = small.clone();
        assert!(fetch(
            &fresh,
            &Http::new(),
            Some(&Restore { remote: &remote, live: &old }),
            &Request { source: &source, version: Some("2026-10-01".into()), params: small.clone() },
            &[]
        )
        .unwrap_err()
        .contains("conflicting retained files"));
        let read = reads(&old).unwrap().remove(0);
        let mut snapshot = fixture.store.snapshot("land", "2026-10-01").unwrap().unwrap();
        snapshot.files.iter_mut().for_each(|f| f.retrieved = "2020-01-01T00:00:00Z".into());
        fixture.store.put_snapshot(&snapshot).unwrap();
        assert_eq!(
            Record::local(&fixture.store, &read).unwrap().unwrap().canonical(),
            old_bytes,
            "local retrieval times do not change immutable records"
        );
    }

    #[test]
    fn empty_and_sibling_requests_restore_without_an_upstream_capture_or_credential() {
        let fixture = fixture("copy-empty");
        fixture.fetched_version("wikidata", "2026-10-01", "landmarks.json", b"capture");
        fixture
            .store
            .put_snapshot(&Snapshot { source: "wikipedia".into(), version: "2026-10-01".into(), files: Vec::new() })
            .unwrap();
        let params = vec![("collection".into(), "landmarks".into()), ("osm".into(), "sha256:old".into())];
        fixture
            .store
            .put_requested(
                "wikipedia",
                &Requested { version: "2026-10-01".into(), params: params.clone(), files: Vec::new() },
            )
            .unwrap();
        let mut live = live(
            &fixture.store,
            &[
                ("wikidata", "2026-10-01", &["landmarks.json"], params.clone()),
                ("wikipedia", "2026-10-01", &[], params.clone()),
            ],
        );
        let dir = fixture.scratch.0.join("bucket");
        copy(&fixture.store, &dir, &mut live);
        let remote = Remote::Bucket(Bucket::local(&dir));
        let copies = Restore { remote: &remote, live: &live };
        let fresh = Store::at(fixture.scratch.0.join("fresh"));
        let mut source = crate::sources::parse_sources(crate::live::tests::LAND).unwrap().remove(0);
        source.id = "wikipedia".into();
        source.fetch.kind = crate::sources::FetchKind::Dtm;
        source.credential = Some(crate::sources::Credential {
            env: Vec::new(),
            file: Some(fixture.scratch.0.join("absent-credential").display().to_string()),
        });
        assert!(!source.credential.as_ref().unwrap().present());
        let request = Request { source: &source, version: Some("2026-10-01".into()), params: params.clone() };
        assert!(fetch(&fresh, &Http::new(), Some(&copies), &request, &[]).unwrap().files.is_empty());
        assert_eq!(
            crate::engine::snapshot_files(&fresh, "wikipedia", "2026-10-01", &params, &[]).unwrap(),
            Some(BTreeMap::new())
        );
        drop(request);
        source.id = "wikidata".into();
        assert_eq!(
            fetch(
                &fresh,
                &Http::new(),
                Some(&copies),
                &Request { source: &source, version: Some("2026-10-01".into()), params: params.clone() },
                &[]
            )
            .unwrap()
            .files
            .len(),
            1
        );
        assert_eq!(fresh.requested("wikipedia", "2026-10-01", &params).unwrap(), Some(Vec::new()));
        assert_eq!(fresh.requested("wikidata", "2026-10-01", &params).unwrap(), Some(vec!["landmarks.json".into()]));
    }

    #[test]
    fn retained_records_and_bytes_fail_closed_and_local_version_conflicts_precede_downloads() {
        let fixture = fixture("copy-conflicts");
        fixture.fetched_version("land", "2026-10-01", "land.zip", b"land");
        let mut live = live(&fixture.store, &[("land", "2026-10-01", &["land.zip"], Vec::new())]);
        let dir = fixture.scratch.0.join("bucket");
        copy(&fixture.store, &dir, &mut live);
        let key = live.inputs.keys().next().unwrap().clone();
        let mut record = live.inputs[&key].clone().unwrap();
        for wrong in ["source", "version", "digest"] {
            let mut bad = record.clone();
            match wrong {
                "source" => bad.source = "other".into(),
                "version" => bad.version = "2026-10-02".into(),
                _ => bad.files[0].sha256 = sha256_hex(b"different"),
            }
            write(&dir.join(key.path()), &String::from_utf8(bad.canonical()).unwrap());
            assert!(read(&Remote::Bucket(Bucket::local(&dir)), &key)
                .unwrap_err()
                .contains("another source, version or digest"));
        }
        let source = crate::sources::parse_sources(crate::live::tests::LAND).unwrap().remove(0);
        let remote = Remote::Bucket(Bucket::local(&dir));
        let copies = Restore { remote: &remote, live: &live };
        let request = Request { source: &source, version: Some("2026-10-01".into()), params: Vec::new() };
        let fresh = Store::at(fixture.scratch.0.join("fresh"));
        let old = FileRecord {
            name: "land.zip".into(),
            url: record.files[0].url.clone(),
            sha256: sha256_hex(b"old"),
            size: 3,
            retrieved: String::new(),
        };
        fresh
            .put_snapshot(&Snapshot { source: source.id.clone(), version: "2026-10-01".into(), files: vec![old] })
            .unwrap();
        assert!(fetch(&fresh, &Http::new(), Some(&copies), &request, &[]).unwrap_err().contains("but the record has"));
        assert!(!fresh.object(&record.files[0].sha256).exists());
        let fresh = Store::at(fixture.scratch.0.join("corrupt"));
        let object = dir.join(format!("{INPUTS}/objects/{}", record.files[0].sha256));
        std::fs::remove_file(&object).unwrap();
        write(&object, "torn");
        assert!(fetch(&fresh, &Http::new(), Some(&copies), &request, &[])
            .unwrap_err()
            .contains("another SHA-256 or size"));
        assert!(fresh.snapshot("land", "2026-10-01").unwrap().is_none());
        live.inputs.insert(key.clone(), None);
        assert!(fetch(&fresh, &Http::new(), Some(&Restore { remote: &remote, live: &live }), &request, &[])
            .unwrap_err()
            .contains("retained input copy is missing"));
        assert_eq!(
            fetch(&fixture.store, &Http::new(), Some(&Restore { remote: &remote, live: &live }), &request, &[])
                .unwrap()
                .files
                .len(),
            1,
            "verified local bytes can repair a deleted remote record"
        );
        record.files.clear();
        assert!(record.validate(&key).is_err(), "a nonempty digest cannot become an empty success");
        let unknown = Read { step: "test/unknown".into(), key, params: Vec::new(), files: Vec::new() };
        assert!(Record::local(&fixture.store, &unknown).unwrap_err().contains("exact input file names"));
    }
}
