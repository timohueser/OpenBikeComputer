//! The three planner services staged before a publication commit.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::engine::LayerFile;
use serde::{Deserialize, Serialize};

/// Publication configuration is separate from the service code and data identity.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Origins {
    pub site_origin: String,
    pub api_origin: String,
    pub objects_origin: String,
}

impl Origins {
    pub fn check(&self) -> Result<(), String> {
        for value in [&self.site_origin, &self.api_origin, &self.objects_origin] {
            let host = value.strip_prefix("https://").ok_or("publication needs HTTPS origins")?;
            if host.is_empty() || !host.bytes().all(|byte| byte.is_ascii_alphanumeric() || b".-:[]".contains(&byte)) {
                return Err("publication origin has a path, credentials or unsafe characters".into());
            }
            let authority: ureq::http::uri::Authority = host.parse().map_err(|_| "invalid publication origin")?;
            let suffix = &host[authority.host().len()..];
            if authority.host().is_empty()
                || !suffix.is_empty() && suffix.strip_prefix(':').and_then(|port| port.parse::<u16>().ok()).is_none()
            {
                return Err("invalid publication host or port".into());
            }
        }
        Ok(())
    }

    pub fn objects_url(&self) -> String {
        format!("{}/planner/objects", self.objects_origin)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Service {
    Routing,
    Search,
    Downloads,
}

impl Service {
    pub fn name(self) -> &'static str {
        match self {
            Self::Routing => "routing",
            Self::Search => "search",
            Self::Downloads => "downloads",
        }
    }
}

/// Actual host prerequisites. Absent interpreters cannot satisfy a runtime target.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Host {
    pub triple: String,
    pub glibc: String,
    pub node: Option<String>,
    pub python: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Installed {
    pub service: Service,
    pub id: String,
    pub slot: u8,
    pub binding: String,
}

impl Installed {
    pub fn check(&self) -> Result<(), String> {
        if self.slot > 1
            || self.id.len() != 64
            || !self.id.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            || self.binding.len() != 64
            || !self.binding.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err("invalid service slot or identity".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct State {
    pub host: Host,
    pub installed: Vec<Installed>,
}

/// A verified view of existing release metadata and objects, valid during `Vps::stage`.
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    pub service: Service,
    pub id: String,
    pub target: Host,
    pub expected: Ready,
    pub source: PathBuf,
    pub release: LayerFile,
    pub runtime: LayerFile,
    pub objects_url: String,
    pub site_origin: String,
    pub api_origin: String,
}

impl Candidate {
    pub fn binding(&self) -> String {
        binding(self.service, &self.id, &self.api_origin, &self.site_origin, &self.objects_url)
    }
}

fn binding(service: Service, id: &str, api: &str, site: &str, pool: &str) -> String {
    let pool = if service == Service::Downloads { pool } else { "" };
    crate::store::sha256_hex(&serde_json::to_vec(&[service.name(), id, api, site, pool]).expect("strings serialize"))
}

pub fn current(document: Option<&serde_json::Map<String, serde_json::Value>>) -> Result<Vec<Installed>, String> {
    let Some(document) = document else { return Ok(Vec::new()) };
    let active = document.get("active").ok_or("current planner has no active document")?;
    if active.is_null() {
        return Ok(Vec::new());
    }
    let origins: Origins =
        serde_json::from_value(document.get("origins").cloned().ok_or("current planner has no publication origins")?)
            .map_err(|e| e.to_string())?;
    origins.check()?;
    let installed: Vec<Installed> = serde_json::from_value(active["installed"].clone())
        .map_err(|_| "current planner has no verified service slots")?;
    if installed.len() != 3
        || installed.iter().map(|unit| unit.service).collect::<std::collections::BTreeSet<_>>().len() != 3
    {
        return Err("current planner has conflicting service slots".into());
    }
    for unit in &installed {
        unit.check()?;
        if active["services"][unit.service.name()] != unit.id {
            return Err("current service slot differs from its content identity".into());
        }
        let expected =
            binding(unit.service, &unit.id, &origins.api_origin, &origins.site_origin, &origins.objects_url());
        let url = format!("{}/planner-api/services/{}/{}", origins.api_origin, expected, unit.service.name());
        if unit.binding != expected || active[unit.service.name()] != url {
            return Err("current service endpoint differs from its configuration binding".into());
        }
    }
    Ok(installed)
}

/// Slot choices are consent-visible. The owner repeats this after its exact pointer check.
pub fn document(
    wanted: &mut serde_json::Map<String, serde_json::Value>,
    observed: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Result<Vec<Installed>, String> {
    let origins: Origins =
        serde_json::from_value(wanted.get("origins").cloned().ok_or("planner has no publication origins")?)
            .map_err(|e| e.to_string())?;
    origins.check()?;
    let current = current(observed)?;
    let active =
        wanted.get_mut("active").and_then(serde_json::Value::as_object_mut).ok_or("planner has no active document")?;
    let mut chosen = Vec::new();
    for service in [Service::Routing, Service::Search, Service::Downloads] {
        let id = active
            .get("services")
            .and_then(|services| services[service.name()].as_str())
            .ok_or("planner has no service content identity")?
            .to_string();
        let binding = binding(service, &id, &origins.api_origin, &origins.site_origin, &origins.objects_url());
        let previous = current.iter().find(|unit| unit.service == service);
        let slot = previous.map_or(0, |unit| if unit.binding == binding { unit.slot } else { 1 - unit.slot });
        let unit = Installed { service, id, slot, binding };
        unit.check()?;
        let url = format!("{}/planner-api/services/{}/{}", origins.api_origin, unit.binding, service.name());
        active.insert(service.name().into(), url.into());
        chosen.push(unit);
    }
    active.insert("installed".into(), serde_json::to_value(&chosen).map_err(|e| e.to_string())?);
    Ok(chosen)
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Stage {
    pub installed: Installed,
    pub candidate: Candidate,
}

/// Identities reported by the opened service data, rather than its requested configuration.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(tag = "service", rename_all = "snake_case", deny_unknown_fields)]
pub enum Ready {
    Routing { package: String },
    Search { grid: String, model: BTreeMap<String, String> },
    Downloads { catalog: String },
}

impl Ready {
    pub fn from_document(service: Service, document: &serde_json::Value) -> Result<Self, String> {
        let text = |value: &serde_json::Value| {
            value.as_str().map(str::to_string).ok_or_else(|| "missing service data identity".to_string())
        };
        Ok(match service {
            Service::Routing => Self::Routing { package: text(&document["routing_package"])? },
            Service::Downloads => {
                Self::Downloads { catalog: text(&document["files"]["offline/catalog.json"]["sha256"])? }
            }
            Service::Search => {
                let region = text(&document["region"])?;
                let files = &document["files"];
                Self::Search {
                    grid: text(&files[format!("search/{region}.grid.json")]["sha256"])?,
                    model: ["labels.json", "tokenizer.json", "model.int8.onnx"]
                        .into_iter()
                        .map(|name| {
                            text(&files[format!("search/model/{name}")]["sha256"]).map(|digest| (name.into(), digest))
                        })
                        .collect::<Result<_, _>>()?,
                }
            }
        })
    }
}

/// Staging starts and checks a candidate. It does not activate traffic or retire another slot.
pub trait Vps {
    /// Resolve selected immutable bytes before admitting a host mutation.
    fn prepare(&mut self, staged: &Stage) -> Result<Stage, String> {
        Ok(staged.clone())
    }
    fn inspect(&mut self) -> Result<State, String>;
    fn stage(&mut self, staged: &Stage) -> Result<(), String>;
    fn probe(&mut self, staged: &Stage) -> Result<Ready, String>;
    fn activate(&mut self, _stages: &[Stage]) -> Result<Vec<Ready>, String> {
        Err("service backend cannot activate public traffic".into())
    }
    fn retire(&mut self, _previous: &[Installed], _current: &[Installed]) -> Result<(), String> {
        Err("service backend cannot retire public traffic".into())
    }
}

/// The current publication supplies active slots. This operation does not activate them.
pub fn stage(candidates: &[Candidate], current: &[Installed], vps: &mut impl Vps) -> Result<Vec<Installed>, String> {
    use std::collections::BTreeSet;
    if candidates.len() != 3 || candidates.iter().map(|candidate| candidate.service).collect::<BTreeSet<_>>().len() != 3
    {
        return Err("staging needs all three planner services".into());
    }
    if current.iter().any(|installed| installed.check().is_err())
        || current.iter().map(|installed| installed.service).collect::<BTreeSet<_>>().len() != current.len()
    {
        return Err("conflicting current service slots".into());
    }
    let state = vps.inspect()?;
    if state.installed.iter().any(|installed| installed.check().is_err())
        || state.installed.iter().map(|installed| (installed.service, installed.slot)).collect::<BTreeSet<_>>().len()
            != state.installed.len()
        || current.iter().any(|active| {
            state
                .installed
                .iter()
                .any(|unit| unit.service == active.service && unit.slot == active.slot && unit != active)
        })
    {
        return Err("service slot ownership differs from the current publication".into());
    }
    for candidate in candidates {
        Installed { service: candidate.service, id: candidate.id.clone(), slot: 0, binding: candidate.binding() }
            .check()?;
        let target = &candidate.target;
        let version = |value: &str| value.split('.').map(str::parse::<u32>).collect::<Result<Vec<_>, _>>().ok();
        if target.triple != state.host.triple
            || version(&state.host.glibc).zip(version(&target.glibc)).is_none_or(|(host, baseline)| host < baseline)
            || candidate.service != Service::Routing && (target.python.is_none() || target.python != state.host.python)
            || candidate.service == Service::Search && (target.node.is_none() || target.node != state.host.node)
        {
            return Err(format!("host prerequisites differ from the {} runtime target", candidate.service.name()));
        }
        if !matches!(
            (candidate.service, &candidate.expected),
            (Service::Routing, Ready::Routing { .. })
                | (Service::Search, Ready::Search { .. })
                | (Service::Downloads, Ready::Downloads { .. })
        ) {
            return Err("candidate readiness service differs".into());
        }
        if !current.iter().any(|unit| unit.service == candidate.service)
            && state.installed.iter().any(|unit| unit.service == candidate.service)
        {
            return Err(format!("{} has a slot with unknown publication ownership", candidate.service.name()));
        }
    }
    let mut result = Vec::new();
    for candidate in candidates {
        let active = current.iter().find(|unit| unit.service == candidate.service);
        let installed = state
            .installed
            .iter()
            .filter(|unit| {
                unit.service == candidate.service && unit.id == candidate.id && unit.binding == candidate.binding()
            })
            .min_by_key(|unit| active != Some(*unit));
        if let Some(installed) = installed {
            if vps.probe(&Stage { installed: installed.clone(), candidate: candidate.clone() })? != candidate.expected {
                return Err(format!("installed {} reports different data", candidate.service.name()));
            }
            result.push(installed.clone());
            continue;
        }
        let installed = Installed {
            service: candidate.service,
            id: candidate.id.clone(),
            binding: candidate.binding(),
            slot: active.map_or(0, |unit| 1 - unit.slot),
        };
        let prepared = vps.prepare(&Stage { installed: installed.clone(), candidate: candidate.clone() })?;
        if prepared.installed != installed || prepared.candidate.binding() != candidate.binding() {
            return Err("prepared service changes the approved slot or endpoint binding".into());
        }
        vps.stage(&prepared)?;
        if vps.probe(&prepared)? != candidate.expected {
            return Err(format!("candidate {} reports different data", candidate.service.name()));
        }
        result.push(installed);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn full_pointer_choices_preserve_unchanged_services_and_bind_configuration_changes() {
        let first = json!({"origins": {"site_origin":"https://site.example", "api_origin":"https://api.example",
            "objects_origin":"https://objects.example"}, "active":{"services":{
                "routing":"a".repeat(64), "search":"b".repeat(64), "downloads":"c".repeat(64)}}});
        let mut original = first.as_object().unwrap().clone();
        let initial = document(&mut original, None).unwrap();
        assert!(initial.iter().all(|unit| unit.slot == 0));
        let mut optional = original.clone();
        optional.get_mut("active").unwrap()["services"]["downloads"] = json!("d".repeat(64));
        let next = document(&mut optional, Some(&original)).unwrap();
        assert_eq!(&next[..2], &initial[..2]);
        assert_eq!(next[2].slot, 1);
        assert_eq!(optional["active"]["routing"], original["active"]["routing"]);
        assert_eq!(optional["active"]["search"], original["active"]["search"]);
        let mut configured = original.clone();
        configured.get_mut("origins").unwrap()["objects_origin"] = json!("https://other.example");
        let moved = document(&mut configured, Some(&original)).unwrap();
        assert_eq!(&moved[..2], &initial[..2]);
        assert_eq!(moved[2].id, initial[2].id);
        assert_ne!(moved[2].binding, initial[2].binding);
        assert_eq!(moved[2].slot, 1);
        let mut corrupted = original.clone();
        corrupted.get_mut("active").unwrap()["routing"] = json!("https://other.example/wrong");
        assert!(current(Some(&corrupted)).unwrap_err().contains("endpoint"));
        for bad in ["https://host/path", "https://host\nunsafe", "https://user@host", "https://host:99999"] {
            let origins = Origins {
                site_origin: bad.into(),
                api_origin: "https://api.example".into(),
                objects_origin: "https://objects.example".into(),
            };
            assert!(origins.check().is_err());
        }
    }
}
