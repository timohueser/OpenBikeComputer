//! Stage receipt-backed services without changing public traffic.

use std::collections::BTreeSet;

use obc_data::engine::release::Release;
use obc_data::engine::LayerFile;
use obc_data::store::{hash_file, Store};
use obc_data::vps::{Candidate, Host, Ready, Service};
use serde_json::Value;

fn checked(store: &Store, file: &LayerFile) -> Result<std::path::PathBuf, String> {
    let path = store.object(&file.sha256);
    if hash_file(&path)? != (file.sha256.clone(), file.size) {
        return Err(format!("installer input `{}` differs from its receipt", file.path));
    }
    Ok(path)
}

fn named(release: &Release, name: &str) -> Result<LayerFile, String> {
    release
        .named
        .iter()
        .find(|file| file.path == name)
        .cloned()
        .ok_or_else(|| format!("missing installer input `{name}`"))
}

fn ready(service: Service, document: &Value) -> Result<Ready, String> {
    let text =
        |value: &Value| value.as_str().map(str::to_string).ok_or_else(|| "missing service data identity".to_string());
    Ok(match service {
        Service::Routing => Ready::Routing { package: text(&document["routing_package"])? },
        Service::Search => {
            let region = text(&document["region"])?;
            let files = &document["files"];
            Ready::Search {
                grid: text(&files[format!("search/{region}.grid.json")]["sha256"])?,
                model: ["labels.json", "tokenizer.json", "model.int8.onnx"]
                    .into_iter()
                    .map(|name| {
                        text(&files[format!("search/model/{name}")]["sha256"]).map(|digest| (name.into(), digest))
                    })
                    .collect::<Result<_, _>>()?,
            }
        }
        Service::Downloads => Ready::Downloads { catalog: text(&document["files"]["offline/catalog.json"]["sha256"])? },
    })
}

/// Metadata and available bytes are checked. The view stays alive during staging.
pub struct Prepared {
    candidates: Vec<Candidate>,
    _view: tempfile::TempDir,
}

impl Prepared {
    pub fn candidates(&self) -> &[Candidate] {
        &self.candidates
    }
}

pub fn prepare(release: &Release, store: &Store, objects_url: &str, site_origin: &str) -> Result<Prepared, String> {
    release.check_named()?;
    let manifest = named(release, "release.json")?;
    let document: Value =
        serde_json::from_slice(&std::fs::read(checked(store, &manifest)?).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let identities = super::runtime::identities(release)?;
    let view = tempfile::tempdir_in(store.root()).map_err(|e| e.to_string())?;
    let mut candidates = Vec::new();
    let owned = release.objects();
    for service in [Service::Routing, Service::Search, Service::Downloads] {
        let name = service.name();
        let id = identities[name].as_str().ok_or_else(|| format!("missing {name} runtime identity"))?.to_string();
        let descriptor = named(release, &format!("runtime/{name}.json"))?;
        let (body, _) = super::runtime::description(name, release, store)?;
        let target: Host =
            serde_json::from_value(body["target"].clone()).map_err(|e| format!("runtime target: {e}"))?;
        let expected = ready(service, &document)?;
        let source = view.path().join(name);
        std::fs::create_dir_all(source.join("objects")).map_err(|e| e.to_string())?;
        for file in [&manifest, &descriptor] {
            let destination = source.join(&file.path);
            std::fs::create_dir_all(destination.parent().unwrap()).map_err(|e| e.to_string())?;
            std::fs::hard_link(checked(store, file)?, destination).map_err(|e| e.to_string())?;
        }
        let prefixes: &[&str] = match service {
            Service::Routing => &["routing/"],
            Service::Search => &["search/"],
            Service::Downloads => &["offline/"],
        };
        let mut objects = BTreeSet::new();
        let payload = &body["payload"];
        objects.insert((
            payload["sha256"].as_str().ok_or("missing runtime payload")?.to_string(),
            payload["bytes"].as_u64().ok_or("missing runtime size")?,
        ));
        for (name, file) in document["files"].as_object().ok_or("missing service files")? {
            if !prefixes.iter().any(|prefix| name.starts_with(prefix)) {
                continue;
            }
            let file = &file["transport"];
            objects.insert((
                file["sha256"].as_str().ok_or("missing service transport")?.to_string(),
                file["bytes"].as_u64().ok_or("missing service size")?,
            ));
        }
        for (sha256, size) in objects {
            if owned.get(sha256.as_str()) != Some(&size) {
                return Err("service input is not owned by this release".into());
            }
            let file = LayerFile { path: format!("objects/{sha256}"), sha256, size };
            if store.object(&file.sha256).exists() {
                std::fs::hard_link(checked(store, &file)?, source.join(&file.path)).map_err(|e| e.to_string())?;
            }
        }
        candidates.push(Candidate {
            service,
            id,
            target,
            expected,
            source,
            release: manifest.clone(),
            runtime: descriptor,
            objects_url: objects_url.into(),
            site_origin: site_origin.into(),
        });
    }
    Ok(Prepared { candidates, _view: view })
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_data::engine::{release::Layer, Client};
    use obc_data::store::sha256_hex;
    use obc_data::vps::{stage, Installed, Stage, State, Vps};
    use serde_json::json;

    struct Fake {
        state: State,
        expected: Vec<Ready>,
        staged: Vec<Installed>,
        failed: Option<Service>,
    }
    impl Vps for Fake {
        fn inspect(&mut self) -> Result<State, String> {
            Ok(self.state.clone())
        }
        fn stage(&mut self, staged: &Stage) -> Result<(), String> {
            let candidate = &staged.candidate;
            assert!(candidate.source.join("release.json").is_file());
            assert!(candidate.source.join(&candidate.runtime.path).is_file());
            let body: Value =
                serde_json::from_slice(&std::fs::read(candidate.source.join(&candidate.runtime.path)).unwrap())
                    .unwrap();
            if !candidate.source.join("objects").join(body["payload"]["sha256"].as_str().unwrap()).is_file() {
                return Err("missing staged runtime bytes".into());
            }
            self.staged.push(staged.installed.clone());
            Ok(())
        }
        fn probe(&mut self, installed: &Installed) -> Result<Ready, String> {
            if self.failed == Some(installed.service) {
                return Err("candidate is not healthy".into());
            }
            Ok(self
                .expected
                .iter()
                .find(|ready| {
                    matches!(
                        (installed.service, ready),
                        (Service::Routing, Ready::Routing { .. })
                            | (Service::Search, Ready::Search { .. })
                            | (Service::Downloads, Ready::Downloads { .. })
                    )
                })
                .unwrap()
                .clone())
        }
    }

    fn layer(step: &str) -> Layer {
        Layer {
            step: step.into(),
            key: String::new(),
            inputs: Vec::new(),
            options: json!({}),
            code: String::new(),
            command: None,
            outputs: Vec::new(),
            digest: sha256_hex(step.as_bytes()),
            files: Vec::new(),
            snapshots: Default::default(),
            client: Client::All,
        }
    }

    #[test]
    fn staging_reuses_healthy_services_and_isolates_changed_downloads_without_activation() {
        let temporary = tempfile::tempdir().unwrap();
        let store = Store::at(temporary.path().join("store"));
        let put = |path: &str, data: &[u8]| {
            let input = temporary.path().join("input");
            std::fs::write(&input, data).unwrap();
            let sha256 = sha256_hex(data);
            store.insert(&input, &sha256).unwrap();
            LayerFile { path: path.into(), size: data.len() as u64, sha256 }
        };
        let mut release = Release {
            product: "planner".into(),
            region: "test".into(),
            optional: Vec::new(),
            named: Vec::new(),
            layers: [
                "planner/routing/grid",
                "planner/search/pois/grid",
                "planner/search/addresses/grid",
                "planner/model/grid",
                "planner/index",
            ]
            .map(layer)
            .into(),
        };
        let mut files = serde_json::Map::new();
        for path in [
            "routing/blocks.json",
            "search/test.grid.json",
            "search/model/labels.json",
            "search/model/tokenizer.json",
            "search/model/model.int8.onnx",
            "offline/catalog.json",
        ] {
            let file = put(path, path.as_bytes());
            files.insert(path.into(), json!({"bytes": file.size, "sha256": file.sha256, "transport": {"bytes": file.size, "sha256": file.sha256, "encoding": "identity"}}));
            release.layers[4].files.push(file);
        }
        let mut document =
            json!({"region": "test", "routing_package": files["routing/blocks.json"]["sha256"], "files": files});
        release.named.push(put("release.json", &serde_json::to_vec(&document).unwrap()));
        for service in [Service::Routing, Service::Search, Service::Downloads] {
            let name = service.name();
            let payload = put(&format!("{name}.tar.gz"), name.as_bytes());
            let mut producer = layer(&format!("planner/runtime/{name}"));
            producer.options = json!({"target": {"triple": "x86_64-unknown-linux-gnu", "glibc": "2.31", "node": "24.0.0", "python": "3.12.0"}});
            let descriptor = put(
                &format!("runtime/{name}.json"),
                &serde_json::to_vec(&json!({"format": 1, "service": name, "target": producer.options["target"],
                "payload": {"path": payload.path, "bytes": payload.size, "sha256": payload.sha256}}))
                .unwrap(),
            );
            let mut recorded = descriptor.clone();
            recorded.path = "runtime.json".into();
            producer.files.push(recorded);
            producer.client = Client::Paths(vec![payload.path.clone()]);
            producer.files.push(payload);
            release.named.push(descriptor);
            release.layers.push(producer);
        }
        release.named.sort_by(|a, b| a.path.cmp(&b.path));
        let expected = [Service::Routing, Service::Search, Service::Downloads]
            .into_iter()
            .map(|service| ready(service, &document).unwrap())
            .collect();
        let mut fake = Fake {
            state: State {
                host: Host {
                    triple: "x86_64-unknown-linux-gnu".into(),
                    glibc: "2.36".into(),
                    node: Some("24.0.0".into()),
                    python: Some("3.12.0".into()),
                },
                installed: Vec::new(),
            },
            expected,
            staged: Vec::new(),
            failed: None,
        };
        let staged = |release: &Release, current: &[Installed], fake: &mut Fake| {
            let prepared = prepare(
                release,
                &store,
                "https://maps.openbikecomputer.com/planner/objects",
                "https://openbikecomputer.com",
            )?;
            stage(prepared.candidates(), current, fake)
        };
        let first = staged(&release, &[], &mut fake).unwrap();
        assert_eq!(fake.staged.len(), 3);
        assert!(first.iter().all(|unit| unit.slot == 0));
        fake.state.installed = first.clone();
        fake.staged.clear();
        std::fs::remove_file(store.object(&sha256_hex(b"routing"))).unwrap();
        assert_eq!(staged(&release, &first, &mut fake).unwrap(), first);
        assert!(fake.staged.is_empty());
        let offline = put("offline/catalog.json", b"catalog with an optional map");
        document["files"][&offline.path] = json!({"bytes": offline.size, "sha256": offline.sha256, "transport": {"bytes": offline.size, "sha256": offline.sha256, "encoding": "identity"}});
        release.layers[4].files.retain(|file| file.path != offline.path);
        release.layers[4].files.push(offline);
        release.named.retain(|file| file.path != "release.json");
        release.named.insert(0, put("release.json", &serde_json::to_vec(&document).unwrap()));
        fake.expected[2] = ready(Service::Downloads, &document).unwrap();
        release.layers[4].digest = sha256_hex(b"index with optional map");
        let changed = staged(&release, &first, &mut fake).unwrap();
        assert_eq!(&changed[..2], &first[..2]);
        assert_eq!(fake.staged, vec![changed[2].clone()]);
        assert_eq!(changed[2].slot, 1);
        fake.failed = Some(Service::Downloads);
        assert!(staged(&release, &first, &mut fake).unwrap_err().contains("not healthy"));
        assert_eq!(fake.state.installed, first, "failed staging leaves current publication ownership unchanged");
        fake.failed = None;
        fake.state.host.python = None;
        assert!(staged(&release, &first, &mut fake).unwrap_err().contains("prerequisites"));
        fake.state.host.python = Some("3.12.0".into());
        assert!(staged(&release, &[], &mut fake).unwrap_err().contains("ownership"));
        release.layers[5].digest = sha256_hex(b"routing runtime needs another slot");
        assert!(staged(&release, &first, &mut fake).unwrap_err().contains("missing staged runtime"));
    }
}
