//! Target-specific service code receipts. Installation readiness stays a required gate.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use obc_data::engine::release::Release;
use obc_data::engine::{Client, Code, LayerFile, Python, Run, Step};
use obc_data::product::{BlockedLayer, Steps};
use obc_data::store::{hash_file, sha256_hex, Store};
use serde::Deserialize;
use serde_json::{json, Value};

const BUILD: &str = "tools/planner_runtime_build.py";
const RECIPE: &str = "data/planner-runtime.toml";
const SERVICES: [&str; 3] = ["routing", "search", "downloads"];

pub(super) fn approval_config(root: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(root.join(RECIPE)).map_err(|error| format!("{RECIPE}: {error}"))?;
    let config: toml::Value = toml::from_str(&text).map_err(|error| format!("{RECIPE}: {error}"))?;
    serde_json::to_value(config).map_err(|error| error.to_string())
}

pub(super) fn binding(step: &Step) -> Result<Option<obc_data::approval::RuntimeBinding>, String> {
    let Some(service) = SERVICES.iter().find(|service| step.name == format!("planner/runtime/{service}")) else {
        return Ok(None);
    };
    let target = step.options.get("target").cloned().ok_or("runtime has no selected target")?;
    let builder = step.options.get("builder").cloned().ok_or("runtime has no selected builder")?;
    if builder["kind"] != "container"
        || !builder["image"].as_str().is_some_and(|image| {
            image
                .strip_prefix("sha256:")
                .is_some_and(|digest| digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit()))
        })
    {
        return Err(format!(
            "automatic approval for native {service} runtime needs an exact prepared execution binding"
        ));
    }
    Ok(Some(obc_data::approval::RuntimeBinding { target, builder }))
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    target: Option<Value>,
    publication: Option<obc_data::vps::Origins>,
}

pub(super) fn publication(root: &Path) -> Result<obc_data::vps::Origins, String> {
    let config: Config =
        toml::from_str(&std::fs::read_to_string(root.join(RECIPE)).map_err(|e| format!("{RECIPE}: {e}"))?)
            .map_err(|e| format!("{RECIPE}: {e}"))?;
    let origins = config.publication.ok_or("configure [publication] origins in data/planner-runtime.toml")?;
    origins.check()?;
    Ok(origins)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Probe {
    builder: Value,
    paths: Vec<String>,
    files: Vec<String>,
}

fn argv() -> Vec<String> {
    [
        "uv",
        "run",
        "--no-project",
        "--no-sync",
        "--offline",
        "--no-python-downloads",
        "python",
        "-m",
        "tools.planner_runtime_build",
    ]
    .map(str::to_string)
    .into()
}

fn probe(root: &Path, service: &str, target: &Value) -> Result<Probe, String> {
    let args = argv();
    let mut child = Command::new(&args[0])
        .args(&args[1..])
        .args(["--probe", service])
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("prepare the offline runtime builder: {e}"))?;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(target).map_err(|e| e.to_string())?)
        .map_err(|e| format!("runtime probe: {e}"))?;
    let output = child.wait_with_output().map_err(|e| format!("runtime probe: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    serde_json::from_slice(&output.stdout).map_err(|e| format!("runtime probe: {e}"))
}

fn listed(root: &Path, mut inspect: impl FnMut(&Path, &str, &Value) -> Result<Probe, String>) -> Steps {
    let config = std::fs::read_to_string(root.join(RECIPE))
        .map_err(|e| format!("{RECIPE}: {e}"))
        .and_then(|text| toml::from_str::<Config>(&text).map_err(|e| format!("{RECIPE}: {e}")))
        .and_then(|config| {
            config
                .target
                .ok_or_else(|| format!("configure and commit [target] in {RECIPE} before building service runtimes"))
        });
    let mut result = Steps { steps: Vec::new(), blocked: Vec::new() };
    for service in SERVICES {
        let wanted = config.clone().map(|mut target| {
            if service != "search" {
                if let Some(target) = target.as_object_mut() {
                    target.remove("node");
                    if service == "routing" {
                        target.remove("python");
                    }
                }
            }
            target
        });
        let prepared = wanted.and_then(|target| inspect(root, service, &target).map(|probe| (target, probe)));
        match prepared {
            Ok((target, probe)) => {
                let name = format!("planner/runtime/{service}");
                let artifact = format!("{service}.tar.gz");
                let mut run = argv();
                run.push("--step".into());
                let mut paths = vec![BUILD.into(), "tools/step_request.py".into()];
                paths.extend(probe.paths);
                result.steps.push(Step {
                    name,
                    inputs: Vec::new(),
                    options: json!({"service": service, "target": target, "builder": probe.builder, "files": probe.files}),
                    code: Code {
                        paths,
                        crates: if service == "routing" { vec!["route-server".into()] } else { Vec::new() },
                        target: (service == "routing").then(|| target["triple"].as_str().unwrap().into()),
                        rust: (service == "routing").then_some(obc_data::engine::Rust::Prepared {
                            profile: obc_data::engine::Profile::Release,
                        }),
                        python: Some(Python::default()),
                        python_packages: (service == "search").then(|| "search-runtime".into()),
                        ..Default::default()
                    },
                    outputs: vec![artifact.clone(), "runtime.json".into()],
                    run: Run::Command(run),
                    client: Client::Paths(vec![artifact]),
                });
            }
            Err(reason) => result.blocked.push(BlockedLayer { layer: format!("planner/runtime/{service}"), reason }),
        }
    }
    if let Err(reason) = publication(root) {
        result.blocked.push(BlockedLayer { layer: "planner/runtime".into(), reason });
    }
    result
}

pub fn steps(root: &Path) -> Steps {
    listed(root, probe)
}

pub fn named(release: &Release) -> Result<Vec<LayerFile>, String> {
    let mut files = Vec::new();
    for service in SERVICES {
        let Some(layer) = release.layers.iter().find(|layer| layer.step == format!("planner/runtime/{service}")) else {
            continue;
        };
        let mut file = layer
            .files
            .iter()
            .find(|file| file.path == "runtime.json")
            .ok_or("runtime receipt has no descriptor")?
            .clone();
        file.path = format!("runtime/{service}.json");
        files.push(file);
    }
    Ok(files)
}

pub fn identities(release: &Release) -> Result<Value, String> {
    let mut result = serde_json::Map::new();
    for service in SERVICES {
        let runtime = format!("planner/runtime/{service}");
        if !release.layers.iter().any(|layer| layer.step == runtime) {
            continue;
        }
        let data: &[&str] = match service {
            "routing" => &["planner/routing/grid"],
            "search" => &["planner/search/pois/grid", "planner/search/addresses/grid", "planner/model/grid"],
            _ => &["planner/index"],
        };
        let mut inputs = std::collections::BTreeMap::new();
        for step in std::iter::once(runtime.as_str()).chain(data.iter().copied()) {
            let layer = release
                .layers
                .iter()
                .find(|layer| layer.step == step)
                .ok_or_else(|| format!("service `{service}` has no input `{step}`"))?;
            inputs.insert(step, layer.digest.as_str());
        }
        result.insert(service.into(), json!(sha256_hex(&serde_json::to_vec(&inputs).map_err(|e| e.to_string())?)));
    }
    Ok(result.into())
}

pub(super) fn description(service: &str, release: &Release, store: &Store) -> Result<(Value, LayerFile), String> {
    let layer = release
        .layers
        .iter()
        .find(|layer| layer.step == format!("planner/runtime/{service}"))
        .ok_or("missing runtime layer")?;
    let descriptor = release
        .named
        .iter()
        .find(|file| file.path == format!("runtime/{service}.json"))
        .ok_or("runtime has no named descriptor")?;
    let recorded =
        layer.files.iter().find(|file| file.path == "runtime.json").ok_or("runtime receipt has no descriptor")?;
    if (descriptor.sha256.as_str(), descriptor.size) != (recorded.sha256.as_str(), recorded.size) {
        return Err("named runtime descriptor differs from its receipt".into());
    }
    let path = store.object(&descriptor.sha256);
    if hash_file(&path)? != (descriptor.sha256.clone(), descriptor.size) {
        return Err("runtime descriptor differs from its receipt".into());
    }
    let body: Value =
        serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let archive = format!("{service}.tar.gz");
    let payload =
        layer.client_files().find(|file| file.path == archive).ok_or("runtime payload is not a client artifact")?;
    if body["format"] != 1
        || body["service"] != service
        || body["target"] != layer.options["target"]
        || body["payload"] != json!({"path": archive, "bytes": payload.size, "sha256": payload.sha256})
    {
        return Err("runtime target or payload differs from its receipt".into());
    }
    Ok((body, payload.clone()))
}

pub fn verify(previous: Option<&Release>, release: &Release, store: &Store) -> Result<(), String> {
    for service in SERVICES {
        let step = format!("planner/runtime/{service}");
        let Some(layer) = release.layers.iter().find(|layer| layer.step == step) else { continue };
        let (_, payload) = description(service, release, store)?;
        let unchanged =
            previous.is_some_and(|old| old.layers.iter().any(|old| old.step == step && old.digest == layer.digest));
        if !unchanged && hash_file(&store.object(&payload.sha256))? != (payload.sha256.clone(), payload.size) {
            return Err("runtime payload differs from its receipt".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::maps::tests::temp;
    use obc_data::engine::release::Layer;

    #[test]
    fn runtime_discovery_uses_the_explicit_checkout_and_keeps_readiness_blocked() {
        let fixture = temp("runtime-target");
        std::fs::create_dir_all(fixture.0.join("data")).unwrap();
        let recipe = fixture.0.join(RECIPE);
        std::fs::write(&recipe, "").unwrap();
        let empty = listed(&fixture.0, |_, _, _| unreachable!("unconfigured targets inspect no tools"));
        assert!(empty.steps.is_empty());
        assert_eq!(empty.blocked.len(), 4);
        std::fs::write(&recipe, "[target]\ntriple = \"x86_64-unknown-linux-gnu\"\nglibc = \"2.31\"\nnode = \"24.0.0\"\npython = \"3.12.0\"\n").unwrap();
        let found = listed(&fixture.0, |root, service, target| {
            assert_eq!(root, fixture.0);
            assert_eq!(target["triple"], "x86_64-unknown-linux-gnu");
            assert_eq!(target.get("node").is_some(), service == "search");
            Ok(Probe { builder: json!({"kind": "native"}), paths: Vec::new(), files: Vec::new() })
        });
        assert_eq!(found.steps.len(), 3);
        assert_eq!(found.blocked.len(), 1);
        assert_eq!(found.blocked[0].layer, "planner/runtime");
        assert_eq!(found.steps[0].code.target.as_deref(), Some("x86_64-unknown-linux-gnu"));
        assert_eq!(
            found.steps[0].code.rust,
            Some(obc_data::engine::Rust::Prepared { profile: obc_data::engine::Profile::Release })
        );
        assert_eq!(found.steps[1].code.python_packages.as_deref(), Some("search-runtime"));
        assert!(found.steps[1].code.python.as_ref().unwrap().group.is_none());
        let target_only = std::fs::read_to_string(&recipe).unwrap();
        std::fs::write(&recipe, format!("{target_only}\n[publication]\nsite_origin='https://site.example'\napi_origin='https://api.example'\nobjects_origin='https://objects.example'\n")).unwrap();
        let configured = listed(&fixture.0, |_, _, _| {
            Ok(Probe { builder: json!({"kind":"native"}), paths: Vec::new(), files: Vec::new() })
        });
        assert!(configured.blocked.is_empty());
        assert_eq!(
            configured.steps.iter().map(|step| &step.options).collect::<Vec<_>>(),
            found.steps.iter().map(|step| &step.options).collect::<Vec<_>>()
        );
        assert_eq!(
            configured.steps.iter().map(|step| &step.code).collect::<Vec<_>>(),
            found.steps.iter().map(|step| &step.code).collect::<Vec<_>>()
        );
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
            client: Client::None,
        }
    }

    #[test]
    fn service_identity_binds_code_and_its_data_without_optional_maps() {
        let mut release = Release {
            product: "planner".into(),
            region: "test".into(),
            optional: Vec::new(),
            named: Vec::new(),
            layers: [
                "planner/runtime/routing",
                "planner/runtime/search",
                "planner/routing/grid",
                "planner/search/pois/grid",
                "planner/search/addresses/grid",
                "planner/model/grid",
                "planner/runtime/downloads",
                "planner/index",
            ]
            .map(layer)
            .into(),
        };
        let first = identities(&release).unwrap();
        release.layers.push(layer("planner/sun/grid"));
        assert_eq!(identities(&release).unwrap(), first);
        release.layers[2].digest = sha256_hex(b"changed routing");
        let routing = identities(&release).unwrap();
        assert_ne!(routing["routing"], first["routing"]);
        assert_eq!(routing["search"], first["search"]);
        release.layers[1].digest = sha256_hex(b"changed search executable");
        let search = identities(&release).unwrap();
        assert_ne!(search["search"], routing["search"]);
        assert_eq!(search["routing"], routing["routing"]);
        release.layers[5].digest = sha256_hex(b"changed model");
        assert_ne!(identities(&release).unwrap()["search"], search["search"]);
        let with_model = identities(&release).unwrap();
        release.layers[7].digest = sha256_hex(b"offline catalog with new optional maps");
        let with_maps = identities(&release).unwrap();
        assert_ne!(with_maps["downloads"], with_model["downloads"]);
        assert_eq!(with_maps["routing"], with_model["routing"]);
        assert_eq!(with_maps["search"], with_model["search"]);
    }

    #[test]
    fn runtime_descriptors_bind_the_target_and_owned_payload() {
        let fixture = temp("runtime-descriptor");
        std::fs::create_dir_all(&fixture.0).unwrap();
        let store = Store::at(fixture.0.join("store"));
        let payload = fixture.0.join("payload");
        std::fs::write(&payload, b"authored receipt payload").unwrap();
        let (sha256, size) = hash_file(&payload).unwrap();
        store.insert(&payload, &sha256).unwrap();
        let mut runtime = layer("planner/runtime/routing");
        runtime.options = json!({"target": {"triple": "x86_64-unknown-linux-gnu", "glibc": "2.31"}});
        runtime.client = Client::Paths(vec!["routing.tar.gz".into()]);
        runtime.files.push(LayerFile { path: "routing.tar.gz".into(), sha256: sha256.clone(), size });
        let body = json!({"format": 1, "service": "routing", "target": runtime.options["target"],
            "payload": {"path": "routing.tar.gz", "sha256": sha256, "bytes": size}});
        let release = |body: &Value| {
            let descriptor = fixture.0.join("descriptor");
            std::fs::write(&descriptor, serde_json::to_vec(body).unwrap()).unwrap();
            let (sha256, size) = hash_file(&descriptor).unwrap();
            store.insert(&descriptor, &sha256).unwrap();
            let mut runtime = runtime.clone();
            runtime.files.push(LayerFile { path: "runtime.json".into(), sha256: sha256.clone(), size });
            Release {
                product: "planner".into(),
                region: "test".into(),
                optional: Vec::new(),
                layers: vec![runtime],
                named: vec![LayerFile { path: "runtime/routing.json".into(), sha256, size }],
            }
        };
        let first = release(&body);
        verify(None, &first, &store).unwrap();
        let mut wrong = body.clone();
        wrong["target"]["triple"] = json!("aarch64-unknown-linux-gnu");
        assert!(verify(None, &release(&wrong), &store).unwrap_err().contains("target or payload"));
        wrong = body.clone();
        wrong["payload"]["sha256"] = json!(sha256_hex(b"unowned bytes"));
        assert!(verify(None, &release(&wrong), &store).unwrap_err().contains("target or payload"));
        let mut private = first.clone();
        private.layers[0].client = Client::None;
        assert!(verify(None, &private, &store).unwrap_err().contains("not a client artifact"));
        let mut unbound = first.clone();
        unbound.named[0].sha256 = sha256_hex(b"another descriptor");
        assert!(verify(None, &unbound, &store).unwrap_err().contains("named runtime descriptor"));
        std::fs::remove_file(store.object(&sha256)).unwrap();
        verify(Some(&first), &first, &store).unwrap();
        assert!(verify(None, &first, &store).is_err(), "a changed payload requires its immutable bytes");
    }
}
