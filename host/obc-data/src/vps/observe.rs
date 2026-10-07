//! Read installed services through a separately authorized read-only owner command.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use super::{Candidate, Installed, Origins, Ready, Service, Stage, State};
use crate::live::{LiveProduct, Remote};
use crate::store::sha256_hex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub host: Option<State>,
    pub services: Vec<ServiceStatus>,
    pub unavailable: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ServiceStatus {
    pub service: Service,
    pub ready: bool,
    pub reason: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    document: serde_json::Value,
    installed: Vec<Installed>,
    candidates: Vec<Candidate>,
}

impl Observation {
    fn unavailable(reason: String) -> Self {
        Self { host: None, services: Vec::new(), unavailable: Some(reason) }
    }
}

fn request(product: &LiveProduct, remote: &Remote) -> Result<Request, String> {
    let (_, release) = product.release.as_ref().ok_or("planner has no published release")?;
    let document = product.document.as_ref().ok_or("planner has no published pointer")?;
    let installed = super::current(Some(document))?;
    if installed.len() != 3 {
        return Err("planner has no three published service slots".into());
    }
    let origins: Origins =
        serde_json::from_value(document.get("origins").cloned().ok_or("missing publication origins")?)
            .map_err(|e| e.to_string())?;
    origins.check()?;
    let metadata = |name: &str| -> Result<(crate::engine::LayerFile, serde_json::Value), String> {
        let file =
            release.named.iter().find(|file| file.path == name).ok_or("published service metadata is missing")?.clone();
        if file.size > 1024 * 1024 {
            return Err("service metadata is too large".into());
        }
        let bytes = remote
            .get(&format!("{}/releases/{}/{}", product.prefix, release.id(), name))?
            .ok_or("published service metadata is unavailable")?;
        if bytes.len() as u64 != file.size || sha256_hex(&bytes) != file.sha256 {
            return Err("published service metadata checksum differs".into());
        }
        Ok((file, serde_json::from_slice(&bytes).map_err(|e| e.to_string())?))
    };
    let (manifest, data) = metadata("release.json")?;
    let candidates = installed
        .iter()
        .map(|unit| {
            let (runtime, body) = metadata(&format!("runtime/{}.json", unit.service.name()))?;
            Ok(Candidate {
                service: unit.service,
                id: unit.id.clone(),
                target: serde_json::from_value(body["target"].clone()).map_err(|e| e.to_string())?,
                expected: Ready::from_document(unit.service, &data)?,
                source: PathBuf::new(),
                release: manifest.clone(),
                runtime,
                objects_url: origins.objects_url(),
                site_origin: origins.site_origin.clone(),
                api_origin: origins.api_origin.clone(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    super::commit::stages(&candidates, &installed)?;
    let files = data["files"]
        .as_object()
        .ok_or("missing published service files")?
        .iter()
        .filter(|(name, _)| ["routing/", "search/", "offline/"].iter().any(|prefix| name.starts_with(prefix)))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect::<serde_json::Map<_, _>>();
    let document = serde_json::json!({"region":data["region"], "files":files});
    Ok(Request { document, installed, candidates })
}

pub fn read(product: Option<&LiveProduct>, remote: &Remote) -> Observation {
    let observe = || -> Result<Observation, String> {
        let host = std::env::var("OBC_STATUS_HOST")
            .map_err(|_| "set OBC_STATUS_HOST to a dedicated forced read-only SSH alias")?;
        if host.is_empty()
            || host.starts_with('-')
            || !host.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._-@".contains(&byte))
        {
            return Err("OBC_STATUS_HOST is not an SSH host".into());
        }
        let input =
            serde_json::to_vec(&request(product.ok_or("no planner product")?, remote)?).map_err(|e| e.to_string())?;
        let mut command = Command::new("ssh");
        command.args([
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectionAttempts=1",
            "-o",
            "ConnectTimeout=10",
            "-o",
            "ServerAliveInterval=5",
            "-o",
            "ServerAliveCountMax=2",
            &host,
            "/opt/obc-data/bin/obc-data-plumbing",
            "live-status",
        ]);
        let bytes = crate::operation::launch::bounded_input(&mut command, &input, Duration::from_secs(45))?;
        let observation: Observation = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        if observation.unavailable.is_none()
            && (observation.host.is_none()
                || observation.services.len() != 3
                || observation
                    .services
                    .iter()
                    .map(|service| service.service)
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    != 3)
        {
            return Err("host observation is incomplete".into());
        }
        Ok(observation)
    };
    observe().unwrap_or_else(Observation::unavailable)
}

pub fn main() -> Result<(), String> {
    if !cfg!(target_os = "linux") {
        return Err("installed VPS observation requires Linux".into());
    }
    let mut input = Vec::new();
    std::io::stdin()
        .take(crate::operation::launch::READ_INPUT_LIMIT as u64 + 1)
        .read_to_end(&mut input)
        .map_err(|e| e.to_string())?;
    if input.len() > crate::operation::launch::READ_INPUT_LIMIT {
        return Err("host observation request is too large".into());
    }
    let request: Request = serde_json::from_slice(&input).map_err(|e| e.to_string())?;
    if request.installed.len() != 3
        || request.installed.iter().map(|unit| unit.service).collect::<std::collections::BTreeSet<_>>().len() != 3
    {
        return Err("read-only status requires the three published slots".into());
    }
    if request.candidates.len() != 3
        || request.candidates.iter().any(|candidate| !candidate.source.as_os_str().is_empty())
    {
        return Err("read-only status accepts only published metadata, without a source directory".into());
    }
    for candidate in &request.candidates {
        Origins {
            site_origin: candidate.site_origin.clone(),
            api_origin: candidate.api_origin.clone(),
            objects_origin: candidate
                .objects_url
                .strip_suffix("/planner/objects")
                .ok_or("invalid published object pool")?
                .into(),
        }
        .check()?;
    }
    let stages: Vec<Stage> = super::commit::stages(&request.candidates, &request.installed)?;
    for stage in stages {
        stage.installed.check()?;
    }
    let mut command = Command::new("python3");
    command.args(["-I", "-S", "-B", "-c", include_str!("observe.py")]).current_dir(Path::new("/"));
    let output = crate::operation::launch::bounded_input(&mut command, &input, Duration::from_secs(35))?;
    let observation: Observation = serde_json::from_slice(&output).map_err(|e| e.to_string())?;
    println!("{}", serde_json::to_string(&observation).map_err(|e| e.to_string())?);
    Ok(())
}
